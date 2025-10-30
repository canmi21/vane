/* engine/src/modules/plugins/manager.rs */

use super::builtin::PLUGINS;
use crate::daemon::config;
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

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
pub fn default_config() -> Value {
	Value::Object(serde_json::Map::new())
}

/// A composite key for identifying plugins, combining name and version.
pub type PluginKey = (String, String);

/// The in-memory store for all plugins.
pub type PluginsStore = HashMap<PluginKey, Plugin>;

/// Structure for the response of the `list_plugins` endpoint.
#[derive(Serialize)]
pub struct AllPluginsResponse {
	pub internal: Vec<Plugin>,
	pub external: Vec<Plugin>,
}

// --- State Management & Logic ---

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
pub async fn save_external_plugins(store: &PluginsStore) -> Result<(), std::io::Error> {
	let path = config::get_plugins_config_path();
	let external_plugins: Vec<&Plugin> = store
		.values()
		.filter(|p| p.source == PluginSource::External)
		.collect();
	let contents = serde_json::to_string_pretty(&external_plugins).unwrap();
	tokio::fs::write(path, contents).await
}
