/* engine/src/modules/plugins/builtin.rs */

use crate::common::response;
use crate::daemon::config;
use axum::{
	Json,
	extract::Path,
	http::StatusCode,
	response::{IntoResponse, Response},
};
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// --- Data Structures ---

/// Represents the source of a plugin, either built-in or user-defined.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginSource {
	Internal,
	External,
}

/// Represents a single plugin with its configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Plugin {
	pub name: String,
	pub version: String,
	pub source: PluginSource,
	#[serde(default = "default_config")]
	pub config: Value,
}

// Provides a default empty JSON object for plugin config.
fn default_config() -> Value {
	Value::Object(serde_json::Map::new())
}

/// A composite key for identifying plugins, combining name and version.
type PluginKey = (String, String);

/// The in-memory store for all plugins.
type PluginsStore = HashMap<PluginKey, Plugin>;

/// Structure for the response of the `list_plugins` endpoint.
#[derive(Serialize)]
pub struct AllPluginsResponse {
	internal: Vec<Plugin>,
	external: Vec<Plugin>,
}

// --- State Management ---

// A lazy-initialized, thread-safe, and shared global store for plugins.
static PLUGINS: Lazy<Arc<RwLock<PluginsStore>>> = Lazy::new(|| {
	// Internal plugins are hardcoded here.
	let mut store = PluginsStore::new();
	let ratelimit_plugin = Plugin {
		name: "ratelimit".to_string(),
		version: "v1".to_string(),
		source: PluginSource::Internal,
		config: serde_json::json!({
			"description": "Provides rate limiting capabilities."
		}),
	};
	store.insert(
		(
			ratelimit_plugin.name.clone(),
			ratelimit_plugin.version.clone(),
		),
		ratelimit_plugin,
	);

	Arc::new(RwLock::new(store))
});

/// Initializes the plugin store by loading external plugins from disk.
/// This should be called once at application startup.
pub async fn initialize_plugins() {
	log(LogLevel::Info, "Initializing plugins...");
	let path = config::get_plugins_config_path();
	let mut plugins = PLUGINS.write().await;

	match tokio::fs::read_to_string(&path).await {
		Ok(content) => {
			let external_plugins: Vec<Plugin> = serde_json::from_str(&content).unwrap_or_else(|e| {
				log(
					LogLevel::Error,
					&format!(
						"Failed to parse plugins.json: {}. Starting with an empty list.",
						e
					),
				);
				Vec::new()
			});

			for mut plugin in external_plugins {
				plugin.source = PluginSource::External; // Ensure source is always external from file
				let key = (plugin.name.clone(), plugin.version.clone());
				if plugins.contains_key(&key) {
					log(
						LogLevel::Warn,
						&format!(
							"External plugin '{}:{}' conflicts with an existing plugin and will be ignored.",
							plugin.name, plugin.version
						),
					);
				} else {
					plugins.insert(key, plugin);
				}
			}
		}
		Err(_) => {
			log(
				LogLevel::Debug,
				"plugins.json not found. Creating a new one.",
			);
			// Attempt to save an empty list to create the file.
			if let Err(e) = save_external_plugins(&plugins).await {
				log(
					LogLevel::Error,
					&format!("Failed to create initial plugins.json: {}", e),
				);
			}
		}
	}
	log(LogLevel::Info, "Plugins initialized.");
}

/// Saves only the external plugins to the plugins.json file.
async fn save_external_plugins(store: &PluginsStore) -> Result<(), std::io::Error> {
	let path = config::get_plugins_config_path();
	let external_plugins: Vec<&Plugin> = store
		.values()
		.filter(|p| p.source == PluginSource::External)
		.collect();
	let contents = serde_json::to_string_pretty(&external_plugins).unwrap();
	tokio::fs::write(path, contents).await
}

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
	// Create the key tuple, moving name and version.
	let key = (name, version);
	match plugins.get(&key) {
		Some(plugin) => response::success(plugin).into_response(),
		None => {
			log(
				LogLevel::Warn,
				// Use the values from the key tuple for logging.
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

	if let Err(e) = save_external_plugins(&plugins).await {
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

	// This block limits the scope of the mutable borrow.
	{
		let existing_plugin = match plugins.get_mut(&key) {
			Some(plugin) => plugin,
			None => {
				log(
					LogLevel::Warn,
					&format!("Plugin not found for update: {}:{}", key.0, key.1),
				);
				return response::error(StatusCode::NOT_FOUND, "Plugin not found.".to_string())
					.into_response();
			}
		};

		if existing_plugin.source == PluginSource::Internal {
			log(
				LogLevel::Warn,
				&format!("Attempted to update internal plugin: {}:{}", key.0, key.1),
			);
			return response::error(
				StatusCode::FORBIDDEN,
				"Internal plugins cannot be modified.".to_string(),
			)
			.into_response();
		}

		// Update the config if provided in the payload.
		if let Some(new_config) = payload.config {
			existing_plugin.config = new_config;
		}

		// Clone the data for the response before the mutable borrow ends.
		plugin_for_response = existing_plugin.clone();
	} // The mutable borrow of `plugins` from `get_mut` ends here.

	if let Err(e) = save_external_plugins(&plugins).await {
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
			log(
				LogLevel::Warn,
				&format!("Attempted to delete internal plugin: {}:{}", key.0, key.1),
			);
			return response::error(
				StatusCode::FORBIDDEN,
				"Internal plugins cannot be deleted.".to_string(),
			)
			.into_response();
		}
		Some(_) => {
			// Plugin exists and is external, so it's safe to proceed.
		}
		None => {
			log(
				LogLevel::Warn,
				&format!("Plugin not found for deletion: {}:{}", key.0, key.1),
			);
			return response::error(StatusCode::NOT_FOUND, "Plugin not found.".to_string())
				.into_response();
		}
	}

	// Remove the plugin from the in-memory store.
	plugins.remove(&key);

	if let Err(e) = save_external_plugins(&plugins).await {
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
