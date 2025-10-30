/* engine/src/modules/plugins/handler.rs */

use super::{
	builtin::PLUGINS,
	manager::{self, AllPluginsResponse, Plugin, PluginSource, default_config},
};
use crate::common::response;
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use serde::Deserialize;
use serde_json::Value;

// --- API Payloads ---

#[derive(Deserialize, Debug)]
pub struct PluginPayload {
	pub config: Option<Value>,
}

// --- Axum Handlers ---

/// Lists all registered plugins, separated by internal and external.
pub async fn list_plugins() -> impl IntoResponse {
	log(LogLevel::Debug, "GET /v1/plugins called");
	let plugins = PLUGINS.read().await;
	let mut response = AllPluginsResponse {
		internal: Vec::new(),
		external: Vec::new(),
	};

	for plugin in plugins.values() {
		match plugin.source {
			PluginSource::Internal => response.internal.push(plugin.clone()),
			PluginSource::External => response.external.push(plugin.clone()),
		}
	}
	response::success(response)
}

/// Retrieves a specific plugin by its name and version.
pub async fn get_plugin(Path((name, version)): Path<(String, String)>) -> Response {
	log(
		LogLevel::Debug,
		&format!("GET /v1/plugins/{}/{} called", name, version),
	);
	let plugins = PLUGINS.read().await;
	let key = (name, version);
	match plugins.get(&key) {
		Some(plugin) => response::success(plugin).into_response(),
		None => {
			log(
				LogLevel::Warn,
				&format!("Plugin not found: {}:{}", key.0, key.1),
			);
			response::error(StatusCode::NOT_FOUND, "Plugin not found.".to_string()).into_response()
		}
	}
}

/// Creates a new external plugin.
pub async fn create_plugin(
	Path((name, version)): Path<(String, String)>,
	Json(payload): Json<PluginPayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("POST /v1/plugins/{}/{} called", name, version),
	);
	let mut plugins = PLUGINS.write().await;
	let key = (name.clone(), version.clone());

	if plugins.contains_key(&key) {
		log(
			LogLevel::Warn,
			&format!("Attempted to create existing plugin: {}:{}", name, version),
		);
		return response::error(
			StatusCode::CONFLICT,
			"A plugin with this name and version already exists.".to_string(),
		)
		.into_response();
	}

	let new_plugin = Plugin {
		name,
		version,
		source: PluginSource::External,
		config: payload.config.unwrap_or_else(default_config),
	};

	plugins.insert(key, new_plugin.clone());

	if let Err(e) = manager::save_external_plugins(&plugins).await {
		log(
			LogLevel::Error,
			&format!("Failed to save plugin after creation: {}", e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to save plugin to disk.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Plugin created: {}:{}", new_plugin.name, new_plugin.version),
	);
	(StatusCode::CREATED, Json(new_plugin)).into_response()
}

/// Updates an existing external plugin.
pub async fn update_plugin(
	Path((name, version)): Path<(String, String)>,
	Json(payload): Json<PluginPayload>,
) -> Response {
	log(
		LogLevel::Info,
		&format!("PUT /v1/plugins/{}/{} called", name, version),
	);
	let mut plugins = PLUGINS.write().await;
	let key = (name, version);
	let plugin_for_response;

	{
		let existing_plugin = match plugins.get_mut(&key) {
			Some(plugin) => plugin,
			None => {
				return response::error(StatusCode::NOT_FOUND, "Plugin not found.".to_string())
					.into_response();
			}
		};

		if existing_plugin.source == PluginSource::Internal {
			return response::error(
				StatusCode::FORBIDDEN,
				"Internal plugins cannot be modified.".to_string(),
			)
			.into_response();
		}

		if let Some(new_config) = payload.config {
			existing_plugin.config = new_config;
		}
		plugin_for_response = existing_plugin.clone();
	}

	if let Err(e) = manager::save_external_plugins(&plugins).await {
		log(
			LogLevel::Error,
			&format!("Failed to save updated plugin {}:{}", key.0, e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to save updated plugin to disk.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Plugin updated: {}:{}", key.0, key.1),
	);
	response::success(plugin_for_response).into_response()
}

/// Deletes an external plugin.
pub async fn delete_plugin(Path((name, version)): Path<(String, String)>) -> Response {
	log(
		LogLevel::Info,
		&format!("DELETE /v1/plugins/{}/{} called", name, version),
	);
	let mut plugins = PLUGINS.write().await;
	let key = (name, version);

	match plugins.get(&key) {
		Some(plugin) if plugin.source == PluginSource::Internal => {
			return response::error(
				StatusCode::FORBIDDEN,
				"Internal plugins cannot be deleted.".to_string(),
			)
			.into_response();
		}
		Some(_) => {}
		None => {
			return response::error(StatusCode::NOT_FOUND, "Plugin not found.".to_string())
				.into_response();
		}
	}

	plugins.remove(&key);

	if let Err(e) = manager::save_external_plugins(&plugins).await {
		log(
			LogLevel::Error,
			&format!("Failed to save after deleting plugin {}:{}", key.0, e),
		);
		return response::error(
			StatusCode::INTERNAL_SERVER_ERROR,
			"Failed to save changes after deleting plugin.".to_string(),
		)
		.into_response();
	}

	log(
		LogLevel::Info,
		&format!("Plugin deleted: {}:{}", key.0, key.1),
	);
	StatusCode::NO_CONTENT.into_response()
}
