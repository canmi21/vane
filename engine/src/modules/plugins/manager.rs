/* engine/src/modules/plugins/manager.rs */

use super::builtin::PLUGINS;
use crate::daemon::config;
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- Data Structures ---

/// Defines the type of interface for the plugin.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PluginInterface {
	#[serde(rename = "type")]
	pub r#type: String, // e.g., "internal" or "external"
}

/// Defines an input parameter for a plugin.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ParamDefinition {
	#[serde(rename = "type")]
	pub r#type: String, // e.g., "string", "number", "boolean"
}

/// Defines an output variable that a plugin can set.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VariableDefinition {
	#[serde(rename = "type")]
	pub r#type: String, // e.g., "string", "number"
}

/// Defines a plugin author with their name and a URL.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Author {
	pub name: String,
	pub url: String,
}

/// Defines the possible outcomes and variables set by a plugin.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct OutputResults {
	/// A list of possible execution branches. Empty for terminal nodes.
	#[serde(default)]
	pub tree: Vec<String>,

	/// A map of variables that can be passed to subsequent plugins.
	#[serde(default)]
	pub variables: HashMap<String, VariableDefinition>,

	/// If true, indicates this is a terminal node that ends the request flow.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	#[serde(default)]
	pub r#return: bool,
}

/// Represents a single, detailed plugin definition.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Plugin {
	pub name: String,
	pub version: String,
	pub interface: PluginInterface,
	pub description: String,
	pub authors: Vec<Author>, // Replaced author and url
	pub input_params: HashMap<String, ParamDefinition>,
	#[serde(default)]
	pub output_results: OutputResults,
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

			for plugin in external_plugins {
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
		.filter(|p| p.interface.r#type != "internal")
		.collect();
	let contents = serde_json::to_string_pretty(&external_plugins).unwrap();
	tokio::fs::write(path, contents).await
}
