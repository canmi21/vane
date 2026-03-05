/* src/api/handlers/flow.rs */

use crate::response;
use crate::schemas::flow::{
	FlowConfig, FlowConfigData, FlowConfigResponse, FlowConfigWritten, FlowConfigWrittenResponse,
	ValidateQuery, ValidationResult, ValidationResultResponse,
};
use crate::utils::config_file::{self, ConfigFileResult};
use axum::{
	Json,
	extract::{Path, Query},
	http::StatusCode,
	response::IntoResponse,
};
use vane_engine::engine::interfaces::Layer;
use vane_engine::shared::validator;
use vane_primitives::common::config::file_loader;

// --- Handlers ---

/// Get flow configuration
#[utoipa::path(
    get,
    path = "/ports/{port}/{protocol}/flow",
    params(
        ("port" = u16, Path, description = "Port number"),
        ("protocol" = String, Path, description = "Protocol (tcp/udp)")
    ),
    responses(
        (status = 200, description = "Flow configuration", body = FlowConfigResponse),
        (status = 404, description = "Config not found"),
        (status = 409, description = "Multiple config formats found")
    ),
    tag = "flow",
    security(("bearer_auth" = []))
)]
pub async fn get_flow_handler(Path((port, protocol)): Path<(u16, String)>) -> impl IntoResponse {
	if protocol != "tcp" && protocol != "udp" {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_path = file_loader::get_config_dir().join(format!("[{port}]/{protocol}"));

	if tokio::fs::metadata(file_loader::get_config_dir().join(format!("[{port}]")))
		.await
		.is_err()
	{
		return response::error(StatusCode::NOT_FOUND, format!("Port {port} not found"));
	}

	match config_file::find_config::<FlowConfig>(&base_path).await {
		ConfigFileResult::NotFound => response::error(
			StatusCode::NOT_FOUND,
			format!("No flow config for port {port} {protocol}"),
		),
		ConfigFileResult::Single {
			format, content, ..
		} => response::success(FlowConfigData {
			source_format: format,
			content,
		}),
		ConfigFileResult::Ambiguous { found } => response::error(
			StatusCode::CONFLICT,
			format!(
				"Multiple config formats found: {}. Use DELETE first or PUT to overwrite.",
				found.join(", ")
			),
		),
		ConfigFileResult::Error(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Read error: {e}"),
		),
	}
}

/// Create flow configuration
#[utoipa::path(
    post,
    path = "/ports/{port}/{protocol}/flow",
    params(
        ("port" = u16, Path, description = "Port number"),
        ("protocol" = String, Path, description = "Protocol (tcp/udp)")
    ),
    request_body = FlowConfig,
    responses(
        (status = 201, description = "Config created", body = FlowConfigWrittenResponse),
        (status = 409, description = "Config already exists"),
        (status = 400, description = "Validation failed")
    ),
    tag = "flow",
    security(("bearer_auth" = []))
)]
pub async fn post_flow_handler(
	Path((port, protocol)): Path<(u16, String)>,
	Json(config): Json<FlowConfig>,
) -> impl IntoResponse {
	if protocol != "tcp" && protocol != "udp" {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_path = file_loader::get_config_dir().join(format!("[{port}]/{protocol}"));

	let port_dir = file_loader::get_config_dir().join(format!("[{port}]"));
	if tokio::fs::metadata(&port_dir).await.is_err() {
		return response::error(StatusCode::NOT_FOUND, format!("Port {port} not found"));
	}

	if !matches!(
		config_file::find_config::<serde_json::Value>(&base_path).await,
		ConfigFileResult::NotFound
	) {
		return response::error(StatusCode::CONFLICT, "Flow config already exists".into());
	}

	if let Err(e) = validator::validate_flow_config(&config.connection, Layer::L4, &protocol) {
		return response::error(StatusCode::BAD_REQUEST, format!("Validation failed: {e}"));
	}

	match config_file::write_json(&base_path, &config).await {
		Ok(path) => {
			let filename = path.file_name().unwrap().to_str().unwrap().to_owned();
			response::created(FlowConfigWritten {
				port,
				protocol,
				written_to: filename,
				converted_from: None,
			})
		}
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Write error: {e}"),
		),
	}
}

/// Update flow configuration
#[utoipa::path(
    put,
    path = "/ports/{port}/{protocol}/flow",
    params(
        ("port" = u16, Path, description = "Port number"),
        ("protocol" = String, Path, description = "Protocol (tcp/udp)"),
        ValidateQuery
    ),
    request_body = FlowConfig,
    responses(
        (status = 200, description = "Config updated", body = FlowConfigWrittenResponse),
        (status = 400, description = "Validation failed")
    ),
    tag = "flow",
    security(("bearer_auth" = []))
)]
pub async fn put_flow_handler(
	Path((port, protocol)): Path<(u16, String)>,
	Query(query): Query<ValidateQuery>,
	Json(config): Json<FlowConfig>,
) -> impl IntoResponse {
	if protocol != "tcp" && protocol != "udp" {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	if let Err(e) = validator::validate_flow_config(&config.connection, Layer::L4, &protocol) {
		return response::error(StatusCode::BAD_REQUEST, format!("Validation failed: {e}"));
	}

	if query.validate_only.unwrap_or(false) {
		return response::success(FlowConfigWritten {
			port,
			protocol: protocol.clone(),
			written_to: "(dry run)".into(),
			converted_from: None,
		});
	}

	let base_path = file_loader::get_config_dir().join(format!("[{port}]/{protocol}"));

	let port_dir = file_loader::get_config_dir().join(format!("[{port}]"));
	if tokio::fs::metadata(&port_dir).await.is_err() {
		return response::error(StatusCode::NOT_FOUND, format!("Port {port} not found"));
	}

	let deleted = config_file::delete_all_formats(&base_path)
		.await
		.unwrap_or(false);

	match config_file::write_json(&base_path, &config).await {
		Ok(path) => {
			let filename = path.file_name().unwrap().to_str().unwrap().to_owned();
			response::success(FlowConfigWritten {
				port,
				protocol,
				written_to: filename,
				converted_from: if deleted {
					Some("unknown".into())
				} else {
					None
				},
			})
		}
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Write error: {e}"),
		),
	}
}

/// Delete flow configuration
#[utoipa::path(
    delete,
    path = "/ports/{port}/{protocol}/flow",
    params(
        ("port" = u16, Path, description = "Port number"),
        ("protocol" = String, Path, description = "Protocol (tcp/udp)")
    ),
    responses(
        (status = 204, description = "Config deleted"),
        (status = 404, description = "Config not found")
    ),
    tag = "flow",
    security(("bearer_auth" = []))
)]
pub async fn delete_flow_handler(Path((port, protocol)): Path<(u16, String)>) -> impl IntoResponse {
	if protocol != "tcp" && protocol != "udp" {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_path = file_loader::get_config_dir().join(format!("[{port}]/{protocol}"));

	match config_file::delete_all_formats(&base_path).await {
		Ok(true) => StatusCode::NO_CONTENT.into_response(),
		Ok(false) => response::error(StatusCode::NOT_FOUND, "Flow config not found".into()),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Delete error: {e}"),
		),
	}
}

/// Validate flow configuration
#[utoipa::path(
    post,
    path = "/ports/{port}/{protocol}/flow/validate",
    params(
        ("port" = u16, Path, description = "Port number"),
        ("protocol" = String, Path, description = "Protocol (tcp/udp)")
    ),
    request_body = FlowConfig,
    responses(
        (status = 200, description = "Validation result", body = ValidationResultResponse),
        (status = 400, description = "Validation failed")
    ),
    tag = "flow",
    security(("bearer_auth" = []))
)]
pub async fn validate_flow_handler(
	Path((_port, protocol)): Path<(u16, String)>,
	Json(config): Json<FlowConfig>,
) -> impl IntoResponse {
	match validator::validate_flow_config(&config.connection, Layer::L4, &protocol) {
		Ok(_) => response::success(ValidationResult {
			valid: true,
			plugins_used: vec![],
			warnings: vec![],
		}),
		Err(e) => response::error(StatusCode::BAD_REQUEST, format!("Validation failed: {e}")),
	}
}
