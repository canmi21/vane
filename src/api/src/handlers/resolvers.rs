/* src/api/handlers/resolvers.rs */

use crate::response;
use crate::schemas::flow::{FlowConfigWritten, FlowConfigWrittenResponse, ValidateQuery};
use crate::schemas::resolvers::{
	ResolverDetail, ResolverDetailResponse, ResolverListData, ResolverListResponse, ResolverSummary,
};
use crate::utils::config_file::{self, ConfigFileResult};
use axum::{
	Json,
	extract::{Path, Query},
	http::StatusCode,
	response::IntoResponse,
};
use validator::Validate;
use vane_engine::config::{ResolverConfig, SUPPORTED_UPGRADE_PROTOCOLS};
use vane_primitives::common::config::file_loader;

// --- Handlers ---

/// List all resolvers
#[utoipa::path(
    get,
    path = "/resolvers",
    responses(
        (status = 200, description = "List of resolvers", body = ResolverListResponse)
    ),
    tag = "resolvers",
    security(("bearer_auth" = []))
)]
pub async fn list_resolvers_handler() -> impl IntoResponse {
	let base_dir = file_loader::get_config_dir().join("resolvers");
	let mut resolvers = Vec::new();

	for protocol in SUPPORTED_UPGRADE_PROTOCOLS {
		let path = base_dir.join(protocol);
		// Check for any config format
		let config_res = config_file::find_config::<serde_json::Value>(&path).await;

		let (active, source_format) = match config_res {
			ConfigFileResult::Single { format, .. } => (true, Some(format)),
			_ => (false, None),
		};

		resolvers.push(ResolverSummary { protocol: protocol.to_string(), active, source_format });
	}

	response::success(ResolverListData {
		resolvers,
		supported_protocols: SUPPORTED_UPGRADE_PROTOCOLS.iter().map(|s| s.to_string()).collect(),
	})
}

/// Get resolver configuration
#[utoipa::path(
    get,
    path = "/resolvers/{protocol}",
    params(
        ("protocol" = String, Path, description = "Protocol (tls, http, quic)")
    ),
    responses(
        (status = 200, description = "Resolver configuration", body = ResolverDetailResponse),
        (status = 404, description = "Config not found"),
        (status = 409, description = "Multiple config formats found")
    ),
    tag = "resolvers",
    security(("bearer_auth" = []))
)]
pub async fn get_resolver_handler(Path(protocol): Path<String>) -> impl IntoResponse {
	if !SUPPORTED_UPGRADE_PROTOCOLS.contains(&protocol.as_str()) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_path = file_loader::get_config_dir().join("resolvers").join(&protocol);

	match config_file::find_config::<ResolverConfig>(&base_path).await {
		ConfigFileResult::NotFound => {
			response::error(StatusCode::NOT_FOUND, format!("No config for {protocol}"))
		}
		ConfigFileResult::Single { format, content, .. } => response::success(ResolverDetail {
			protocol,
			source_format: format,
			connection: content.connection,
		}),
		ConfigFileResult::Ambiguous { found } => response::error(
			StatusCode::CONFLICT,
			format!(
				"Multiple formats found: {}. Use DELETE first or PUT to overwrite.",
				found.join(", ")
			),
		),
		ConfigFileResult::Error(e) => {
			response::error(StatusCode::INTERNAL_SERVER_ERROR, format!("Read error: {e}"))
		}
	}
}

/// Create resolver configuration
#[utoipa::path(
    post,
    path = "/resolvers/{protocol}",
    params(
        ("protocol" = String, Path, description = "Protocol name")
    ),
    request_body = ResolverConfig,
    responses(
        (status = 201, description = "Config created", body = FlowConfigWrittenResponse),
        (status = 400, description = "Validation failed"),
        (status = 409, description = "Config already exists")
    ),
    tag = "resolvers",
    security(("bearer_auth" = []))
)]
pub async fn post_resolver_handler(
	Path(protocol): Path<String>,
	Json(config): Json<ResolverConfig>,
) -> impl IntoResponse {
	if !SUPPORTED_UPGRADE_PROTOCOLS.contains(&protocol.as_str()) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_dir = file_loader::get_config_dir().join("resolvers");
	if tokio::fs::metadata(&base_dir).await.is_err() {
		let _ = tokio::fs::create_dir_all(&base_dir).await;
	}
	let base_path = base_dir.join(&protocol);

	if !matches!(
		config_file::find_config::<serde_json::Value>(&base_path).await,
		ConfigFileResult::NotFound
	) {
		return response::error(StatusCode::CONFLICT, "Config already exists".into());
	}

	// Validate (manually set protocol since it's transient)
	let mut config_to_validate = config.clone();
	config_to_validate.protocol = protocol.clone();

	if let Err(e) = config_to_validate.validate() {
		return response::error(StatusCode::BAD_REQUEST, format!("Validation failed: {e}"));
	}

	match config_file::write_json(&base_path, &config).await {
		Ok(path) => {
			let filename = path.file_name().unwrap().to_str().unwrap().to_owned();
			response::created(FlowConfigWritten {
				port: 0, // Not applicable
				protocol,
				written_to: filename,
				converted_from: None,
			})
		}
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, format!("Write error: {e}")),
	}
}

/// Update resolver configuration
#[utoipa::path(
    put,
    path = "/resolvers/{protocol}",
    params(
        ("protocol" = String, Path, description = "Protocol name"),
        ValidateQuery
    ),
    request_body = ResolverConfig,
    responses(
        (status = 200, description = "Config updated", body = FlowConfigWrittenResponse),
        (status = 400, description = "Validation failed")
    ),
    tag = "resolvers",
    security(("bearer_auth" = []))
)]
pub async fn put_resolver_handler(
	Path(protocol): Path<String>,
	Query(query): Query<ValidateQuery>,
	Json(config): Json<ResolverConfig>,
) -> impl IntoResponse {
	if !SUPPORTED_UPGRADE_PROTOCOLS.contains(&protocol.as_str()) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let mut config_to_validate = config.clone();
	config_to_validate.protocol = protocol.clone();

	if let Err(e) = config_to_validate.validate() {
		return response::error(StatusCode::BAD_REQUEST, format!("Validation failed: {e}"));
	}

	if query.validate_only.unwrap_or(false) {
		return response::success(FlowConfigWritten {
			port: 0,
			protocol: protocol.clone(),
			written_to: "(dry run)".into(),
			converted_from: None,
		});
	}

	let base_dir = file_loader::get_config_dir().join("resolvers");
	if tokio::fs::metadata(&base_dir).await.is_err() {
		let _ = tokio::fs::create_dir_all(&base_dir).await;
	}
	let base_path = base_dir.join(&protocol);

	let deleted = config_file::delete_all_formats(&base_path).await.unwrap_or(false);

	match config_file::write_json(&base_path, &config).await {
		Ok(path) => {
			let filename = path.file_name().unwrap().to_str().unwrap().to_owned();
			response::success(FlowConfigWritten {
				port: 0,
				protocol,
				written_to: filename,
				converted_from: if deleted { Some("unknown".into()) } else { None },
			})
		}
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, format!("Write error: {e}")),
	}
}

/// Delete resolver configuration
#[utoipa::path(
    delete,
    path = "/resolvers/{protocol}",
    params(
        ("protocol" = String, Path, description = "Protocol name")
    ),
    responses(
        (status = 204, description = "Config deleted"),
        (status = 404, description = "Config not found")
    ),
    tag = "resolvers",
    security(("bearer_auth" = []))
)]
pub async fn delete_resolver_handler(Path(protocol): Path<String>) -> impl IntoResponse {
	if !SUPPORTED_UPGRADE_PROTOCOLS.contains(&protocol.as_str()) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_path = file_loader::get_config_dir().join("resolvers").join(protocol);

	match config_file::delete_all_formats(&base_path).await {
		Ok(true) => StatusCode::NO_CONTENT.into_response(),
		Ok(false) => response::error(StatusCode::NOT_FOUND, "Config not found".into()),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, format!("Delete error: {e}")),
	}
}
