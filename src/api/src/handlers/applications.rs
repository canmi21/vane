/* src/api/handlers/applications.rs */

use crate::response;
use crate::schemas::applications::{
	ApplicationDetail, ApplicationDetailResponse, ApplicationListData, ApplicationListResponse,
	ApplicationSummary,
};
use crate::schemas::flow::{FlowConfigWritten, FlowConfigWrittenResponse, ValidateQuery};
use crate::utils::config_file::{self, ConfigFileResult};
use axum::{
	Json,
	extract::{Path, Query},
	http::StatusCode,
	response::IntoResponse,
};
use validator::Validate;
use vane_engine::config::{ApplicationConfig, SUPPORTED_APP_PROTOCOLS};
use vane_primitives::common::config::file_loader;

// --- Handlers ---

/// List all applications
#[utoipa::path(
    get,
    path = "/applications",
    responses(
        (status = 200, description = "List of applications", body = ApplicationListResponse)
    ),
    tag = "applications",
    security(("bearer_auth" = []))
)]
pub async fn list_applications_handler() -> impl IntoResponse {
	let base_dir = file_loader::get_config_dir().join("applications");
	let mut applications = Vec::new();

	for protocol in SUPPORTED_APP_PROTOCOLS {
		let path = base_dir.join(protocol);
		let config_res = config_file::find_config::<serde_json::Value>(&path).await;

		let (active, source_format) = match config_res {
			ConfigFileResult::Single { format, .. } => (true, Some(format)),
			_ => (false, None),
		};

		applications.push(ApplicationSummary { protocol: protocol.to_string(), active, source_format });
	}

	response::success(ApplicationListData {
		applications,
		supported_protocols: SUPPORTED_APP_PROTOCOLS.iter().map(|s| s.to_string()).collect(),
	})
}

/// Get application configuration
#[utoipa::path(
    get,
    path = "/applications/{protocol}",
    params(
        ("protocol" = String, Path, description = "Protocol (httpx)")
    ),
    responses(
        (status = 200, description = "Application configuration", body = ApplicationDetailResponse),
        (status = 404, description = "Config not found"),
        (status = 409, description = "Multiple config formats found")
    ),
    tag = "applications",
    security(("bearer_auth" = []))
)]
pub async fn get_application_handler(Path(protocol): Path<String>) -> impl IntoResponse {
	if !SUPPORTED_APP_PROTOCOLS.contains(&protocol.as_str()) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_path = file_loader::get_config_dir().join("applications").join(&protocol);

	match config_file::find_config::<ApplicationConfig>(&base_path).await {
		ConfigFileResult::NotFound => {
			response::error(StatusCode::NOT_FOUND, format!("No config for {protocol}"))
		}
		ConfigFileResult::Single { format, content, .. } => response::success(ApplicationDetail {
			protocol,
			source_format: format,
			pipeline: content.pipeline,
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

/// Create application configuration
#[utoipa::path(
    post,
    path = "/applications/{protocol}",
    params(
        ("protocol" = String, Path, description = "Protocol name")
    ),
    request_body = ApplicationConfig,
    responses(
        (status = 201, description = "Config created", body = FlowConfigWrittenResponse),
        (status = 400, description = "Validation failed"),
        (status = 409, description = "Config already exists")
    ),
    tag = "applications",
    security(("bearer_auth" = []))
)]
pub async fn post_application_handler(
	Path(protocol): Path<String>,
	Json(config): Json<ApplicationConfig>,
) -> impl IntoResponse {
	if !SUPPORTED_APP_PROTOCOLS.contains(&protocol.as_str()) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_dir = file_loader::get_config_dir().join("applications");
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

	let mut config_to_validate = config.clone();
	config_to_validate.protocol = protocol.clone();

	if let Err(e) = config_to_validate.validate() {
		return response::error(StatusCode::BAD_REQUEST, format!("Validation failed: {e}"));
	}

	match config_file::write_json(&base_path, &config).await {
		Ok(path) => {
			let filename = path.file_name().unwrap().to_str().unwrap().to_owned();
			response::created(FlowConfigWritten {
				port: 0,
				protocol,
				written_to: filename,
				converted_from: None,
			})
		}
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, format!("Write error: {e}")),
	}
}

/// Update application configuration
#[utoipa::path(
    put,
    path = "/applications/{protocol}",
    params(
        ("protocol" = String, Path, description = "Protocol name"),
        ValidateQuery
    ),
    request_body = ApplicationConfig,
    responses(
        (status = 200, description = "Config updated", body = FlowConfigWrittenResponse),
        (status = 400, description = "Validation failed")
    ),
    tag = "applications",
    security(("bearer_auth" = []))
)]
pub async fn put_application_handler(
	Path(protocol): Path<String>,
	Query(query): Query<ValidateQuery>,
	Json(config): Json<ApplicationConfig>,
) -> impl IntoResponse {
	if !SUPPORTED_APP_PROTOCOLS.contains(&protocol.as_str()) {
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

	let base_dir = file_loader::get_config_dir().join("applications");
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

/// Delete application configuration
#[utoipa::path(
    delete,
    path = "/applications/{protocol}",
    params(
        ("protocol" = String, Path, description = "Protocol name")
    ),
    responses(
        (status = 204, description = "Config deleted"),
        (status = 404, description = "Config not found")
    ),
    tag = "applications",
    security(("bearer_auth" = []))
)]
pub async fn delete_application_handler(Path(protocol): Path<String>) -> impl IntoResponse {
	if !SUPPORTED_APP_PROTOCOLS.contains(&protocol.as_str()) {
		return response::error(StatusCode::BAD_REQUEST, "Invalid protocol".into());
	}

	let base_path = file_loader::get_config_dir().join("applications").join(protocol);

	match config_file::delete_all_formats(&base_path).await {
		Ok(true) => StatusCode::NO_CONTENT.into_response(),
		Ok(false) => response::error(StatusCode::NOT_FOUND, "Config not found".into()),
		Err(e) => response::error(StatusCode::INTERNAL_SERVER_ERROR, format!("Delete error: {e}")),
	}
}
