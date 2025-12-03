/* src/modules/plugins/handler.rs */

use super::{loader, model::ExternalPluginConfig, registry};
use crate::core::response;
use axum::{Json, extract::Path, http::StatusCode, response::IntoResponse};
use serde_json::json;

/// GET /plugins
pub async fn list_plugins_handler() -> impl IntoResponse {
	let plugins = registry::list_external_plugins();
	let names: Vec<String> = plugins.iter().map(|p| p.name().to_string()).collect();
	response::success(json!({ "plugins": names })).into_response()
}

/// POST /plugins/:name
pub async fn create_plugin_handler(
	Path(name): Path<String>,
	Json(config): Json<ExternalPluginConfig>,
) -> impl IntoResponse {
	if config.name != name {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Path name and body name mismatch.".to_string(),
		)
		.into_response();
	}

	// Check collision
	if registry::get_plugin(&name).is_some() {
		return response::error(
			StatusCode::CONFLICT,
			format!("Plugin '{}' already exists.", name),
		)
		.into_response();
	}

	match loader::register_plugin(config).await {
		Ok(_) => response::success(json!({ "status": "created", "name": name })).into_response(),
		Err(e) => response::error(
			StatusCode::BAD_REQUEST,
			format!("Failed to register plugin: {}", e),
		)
		.into_response(),
	}
}

/// PUT /plugins/:name
pub async fn update_plugin_handler(
	Path(name): Path<String>,
	Json(config): Json<ExternalPluginConfig>,
) -> impl IntoResponse {
	if config.name != name {
		return response::error(
			StatusCode::BAD_REQUEST,
			"Path name and body name mismatch.".to_string(),
		)
		.into_response();
	}

	if registry::get_external_plugin(&name).is_none() {
		return response::error(
			StatusCode::NOT_FOUND,
			format!("External plugin '{}' not found.", name),
		)
		.into_response();
	}

	match loader::register_plugin(config).await {
		Ok(_) => response::success(json!({ "status": "updated", "name": name })).into_response(),
		Err(e) => response::error(
			StatusCode::BAD_REQUEST,
			format!("Failed to update plugin: {}", e),
		)
		.into_response(),
	}
}

/// DELETE /plugins/:name
pub async fn delete_plugin_handler(Path(name): Path<String>) -> impl IntoResponse {
	if registry::get_external_plugin(&name).is_none() {
		return response::error(
			StatusCode::NOT_FOUND,
			format!("External plugin '{}' not found.", name),
		)
		.into_response();
	}

	match loader::delete_plugin(&name) {
		Ok(_) => response::success(json!({ "status": "deleted", "name": name })).into_response(),
		Err(e) => response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			format!("Failed to delete plugin: {}", e),
		)
		.into_response(),
	}
}
