/* src/api/handlers/nodes.rs */

use crate::response;
use crate::schemas::nodes::{
	NodeDetailResponse, NodeListData, NodeListResponse, NodeOperationResponse, NodeOperationResult,
};
use crate::utils::config_file::{self, ConfigFileResult};
use axum::{Json, extract::Path, http::StatusCode, response::IntoResponse};
use validator::Validate;
use vane_primitives::common::config::file_loader;
use vane_primitives::service_discovery::model::{Node, NodesConfig};

// --- Handlers ---

/// List all nodes
#[utoipa::path(
    get,
    path = "/nodes",
    responses(
        (status = 200, description = "List of nodes", body = NodeListResponse)
    ),
    tag = "nodes",
    security(("bearer_auth" = []))
)]
pub async fn list_nodes_handler() -> impl IntoResponse {
	let base_path = file_loader::get_config_dir().join("nodes");

	match config_file::find_config::<NodesConfig>(&base_path).await {
		ConfigFileResult::NotFound => response::success(NodeListData {
			source_format: "none".into(),
			nodes: vec![],
		}),
		ConfigFileResult::Single {
			format, content, ..
		} => response::success(NodeListData {
			source_format: format,
			nodes: content.nodes,
		}),
		ConfigFileResult::Ambiguous { found } => response::error(
			StatusCode::CONFLICT,
			format!(
				"Multiple config formats found for nodes: {}. Use DELETE first or PUT to overwrite.",
				found.join(", ")
			),
		),
		ConfigFileResult::Error(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Read error: {e}"),
		),
	}
}

/// Get node details
#[utoipa::path(
    get,
    path = "/nodes/{name}",
    params(
        ("name" = String, Path, description = "Node name")
    ),
    responses(
        (status = 200, description = "Node details", body = NodeDetailResponse),
        (status = 404, description = "Node not found")
    ),
    tag = "nodes",
    security(("bearer_auth" = []))
)]
pub async fn get_node_handler(Path(name): Path<String>) -> impl IntoResponse {
	let base_path = file_loader::get_config_dir().join("nodes");

	match config_file::find_config::<NodesConfig>(&base_path).await {
		ConfigFileResult::Single { content, .. } => {
			if let Some(node) = content.nodes.into_iter().find(|n| n.name == name) {
				response::success(node)
			} else {
				response::error(StatusCode::NOT_FOUND, format!("Node '{name}' not found"))
			}
		}
		_ => response::error(StatusCode::NOT_FOUND, format!("Node '{name}' not found")),
	}
}

/// Create node
#[utoipa::path(
    post,
    path = "/nodes",
    request_body = Node,
    responses(
        (status = 201, description = "Node created", body = NodeOperationResponse),
        (status = 400, description = "Validation failed"),
        (status = 409, description = "Node already exists")
    ),
    tag = "nodes",
    security(("bearer_auth" = []))
)]
pub async fn create_node_handler(Json(node): Json<Node>) -> impl IntoResponse {
	if let Err(e) = node.validate() {
		return response::error(StatusCode::BAD_REQUEST, format!("Validation failed: {e}"));
	}

	let base_path = file_loader::get_config_dir().join("nodes");
	let mut config = match config_file::find_config::<NodesConfig>(&base_path).await {
		ConfigFileResult::Single { content, .. } => content,
		ConfigFileResult::NotFound => NodesConfig::default(),
		ConfigFileResult::Ambiguous { found } => {
			return response::error(
				StatusCode::CONFLICT,
				format!("Multiple config formats found: {found:?}"),
			);
		}
		ConfigFileResult::Error(e) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Read error: {e}"),
			);
		}
	};

	if config.nodes.iter().any(|n| n.name == node.name) {
		return response::error(
			StatusCode::CONFLICT,
			format!("Node '{}' already exists", node.name),
		);
	}

	config.nodes.push(node.clone());

	// Write back
	match config_file::write_json(&base_path, &config).await {
		Ok(_) => response::created(NodeOperationResult { name: node.name }),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Write error: {e}"),
		),
	}
}

/// Update node
#[utoipa::path(
    put,
    path = "/nodes/{name}",
    params(
        ("name" = String, Path, description = "Node name")
    ),
    request_body = Node,
    responses(
        (status = 200, description = "Node updated", body = NodeOperationResponse),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Node not found")
    ),
    tag = "nodes",
    security(("bearer_auth" = []))
)]
pub async fn update_node_handler(
	Path(name): Path<String>,
	Json(node): Json<Node>,
) -> impl IntoResponse {
	if name != node.name {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Path name and body name mismatch".into(),
		);
	}

	if let Err(e) = node.validate() {
		return response::error(StatusCode::BAD_REQUEST, format!("Validation failed: {e}"));
	}

	let base_path = file_loader::get_config_dir().join("nodes");
	let mut config = match config_file::find_config::<NodesConfig>(&base_path).await {
		ConfigFileResult::Single { content, .. } => content,
		ConfigFileResult::NotFound => {
			return response::error(StatusCode::NOT_FOUND, format!("Node '{name}' not found"));
		}
		ConfigFileResult::Ambiguous { found } => {
			return response::error(
				StatusCode::CONFLICT,
				format!("Multiple config formats found: {found:?}"),
			);
		}
		ConfigFileResult::Error(e) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Read error: {e}"),
			);
		}
	};

	let mut found = false;
	for n in &mut config.nodes {
		if n.name == name {
			*n = node.clone();
			found = true;
			break;
		}
	}

	if !found {
		return response::error(StatusCode::NOT_FOUND, format!("Node '{name}' not found"));
	}

	// Always write as JSON, replacing potential YAML/TOML if it was single
	match config_file::delete_all_formats(&base_path).await {
		Ok(_) => match config_file::write_json(&base_path, &config).await {
			Ok(_) => response::success(NodeOperationResult { name }),
			Err(e) => response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Write error: {e}"),
			),
		},
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Delete error: {e}"),
		),
	}
}

/// Delete node
#[utoipa::path(
    delete,
    path = "/nodes/{name}",
    params(
        ("name" = String, Path, description = "Node name")
    ),
    responses(
        (status = 200, description = "Node deleted", body = NodeOperationResponse),
        (status = 404, description = "Node not found")
    ),
    tag = "nodes",
    security(("bearer_auth" = []))
)]
pub async fn delete_node_handler(Path(name): Path<String>) -> impl IntoResponse {
	let base_path = file_loader::get_config_dir().join("nodes");
	let mut config = match config_file::find_config::<NodesConfig>(&base_path).await {
		ConfigFileResult::Single { content, .. } => content,
		ConfigFileResult::NotFound => {
			return response::error(StatusCode::NOT_FOUND, format!("Node '{name}' not found"));
		}
		ConfigFileResult::Ambiguous { found } => {
			return response::error(
				StatusCode::CONFLICT,
				format!("Multiple config formats found: {found:?}"),
			);
		}
		ConfigFileResult::Error(e) => {
			return response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Read error: {e}"),
			);
		}
	};

	let initial_len = config.nodes.len();
	config.nodes.retain(|n| n.name != name);

	if config.nodes.len() == initial_len {
		return response::error(StatusCode::NOT_FOUND, format!("Node '{name}' not found"));
	}

	// Write back
	match config_file::delete_all_formats(&base_path).await {
		Ok(_) => match config_file::write_json(&base_path, &config).await {
			Ok(_) => response::success(NodeOperationResult { name }),
			Err(e) => response::error(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Write error: {e}"),
			),
		},
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Delete error: {e}"),
		),
	}
}
