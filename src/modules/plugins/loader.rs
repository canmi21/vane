/* src/modules/plugins/loader.rs */

use super::{
	external::ExternalPlugin,
	model::{ExternalPluginConfig, Plugin},
	registry,
};
use crate::common::{getconf, getenv};
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

const PLUGINS_CONFIG_FILE: &str = "plugins.json";

/// Loads external plugins from disk and registers them.
/// Returns the number of plugins loaded.
pub fn initialize() -> usize {
	let config_path = getconf::get_config_dir().join(PLUGINS_CONFIG_FILE);

	if !config_path.exists() {
		// If file doesn't exist, create an empty map json
		let empty: HashMap<String, ExternalPluginConfig> = HashMap::new();
		if let Ok(content) = serde_json::to_string_pretty(&empty) {
			let _ = fs::write(&config_path, content);
		}
		return 0;
	}

	let mut content = match fs::read_to_string(&config_path) {
		Ok(c) => c,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to read {}: {}", PLUGINS_CONFIG_FILE, e),
			);
			return 0;
		}
	};

	// Fix empty file issue: If file is created empty (0 bytes), inject default JSON.
	if content.trim().is_empty() {
		log(
			LogLevel::Debug,
			&format!(
				"⚙ Found empty {}, initializing with default JSON.",
				PLUGINS_CONFIG_FILE
			),
		);
		let empty_json = "{}";
		if let Err(e) = fs::write(&config_path, empty_json) {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to write default json to {}: {}",
					PLUGINS_CONFIG_FILE, e
				),
			);
		}
		content = empty_json.to_string();
	}

	let configs: HashMap<String, ExternalPluginConfig> = match serde_json::from_str(&content) {
		Ok(m) => m,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to parse {}: {}", PLUGINS_CONFIG_FILE, e),
			);
			return 0;
		}
	};

	let registry_map = DashMap::new();
	let mut count = 0;

	for (name, config) in configs {
		let plugin = ExternalPlugin::new(config);
		// Note: We skip blocking validation during startup to ensure the daemon starts even if an endpoint is down.
		// Runtime failures will be handled by the execution logic.
		registry_map.insert(name.clone(), Arc::new(plugin) as Arc<dyn Plugin>);
		count += 1;
	}

	registry::load_external_plugins(registry_map);

	if count > 0 {
		log(
			LogLevel::Info,
			&format!("✓ Loaded {} external plugins.", count),
		);
		start_background_health_check();
	}

	count
}

/// Spans a background task to periodically check the connectivity of external plugins.
fn start_background_health_check() {
	tokio::spawn(async move {
		let interval_str = getenv::get_env("EXTERNAL_PLUGIN_CHECK_INTERVAL_MINS", "15".to_string());
		let interval_mins = interval_str.parse::<u64>().unwrap_or(15);
		let mut interval = tokio::time::interval(Duration::from_secs(interval_mins * 60));

		loop {
			interval.tick().await;
			log(
				LogLevel::Debug,
				"⚙ Running background health check for external plugins...",
			);

			let plugins = registry::list_external_plugins();
			for plugin in plugins {
				let name = plugin.name().to_string();
				// Downcast to ExternalPlugin to access validate_connectivity
				if let Some(ext_plugin) = plugin.as_any().downcast_ref::<ExternalPlugin>() {
					match ext_plugin.validate_connectivity().await {
						Ok(_) => {
							registry::EXTERNAL_PLUGIN_STATUS.insert(name, Ok(()));
						}
						Err(e) => {
							log(
								LogLevel::Warn,
								&format!("⚠ External plugin '{}' is unreachable: {}", name, e),
							);
							registry::EXTERNAL_PLUGIN_STATUS.insert(name, Err(e.to_string()));
						}
					}
				}
			}
		}
	});
}

/// Saves the given config map to disk.
fn save_to_disk(configs: &HashMap<String, ExternalPluginConfig>) -> Result<()> {
	let config_path = getconf::get_config_dir().join(PLUGINS_CONFIG_FILE);
	let content = serde_json::to_string_pretty(configs)?;
	fs::write(config_path, content)?;
	Ok(())
}

/// Adds or Updates a single external plugin.
/// This includes validation and persistence.
pub async fn register_plugin(config: ExternalPluginConfig) -> Result<()> {
	// 1. Check for name collision with Internal plugins (Security)
	if registry::get_internal_plugin(&config.name).is_some() {
		return Err(anyhow!(
			"Plugin name '{}' conflicts with a built-in plugin.",
			config.name
		));
	}

	// 2. Create and Validate
	let plugin = ExternalPlugin::new(config.clone());
	plugin.validate_connectivity().await?;

	// 3. Load current configs from disk (Source of Truth for persistence)
	let config_path = getconf::get_config_dir().join(PLUGINS_CONFIG_FILE);
	let content = fs::read_to_string(&config_path).unwrap_or_else(|_| "{}".to_string());

	// Handle empty file case for runtime updates as well
	let effective_content = if content.trim().is_empty() {
		"{}"
	} else {
		&content
	};

	let mut configs: HashMap<String, ExternalPluginConfig> =
		serde_json::from_str(effective_content).unwrap_or_default();

	// 4. Update Persistence
	configs.insert(config.name.clone(), config);
	save_to_disk(&configs)?;

	// 5. Update Runtime Registry
	// We rebuild the DashMap from the new full config to ensure sync.
	let registry_map = DashMap::new();
	for (name, cfg) in configs {
		registry_map.insert(name, Arc::new(ExternalPlugin::new(cfg)) as Arc<dyn Plugin>);
	}
	registry::load_external_plugins(registry_map);

	log(
		LogLevel::Info,
		&format!("➜ External plugin registered: {}", plugin.name()),
	);
	Ok(())
}

/// Removes an external plugin.
pub fn delete_plugin(name: &str) -> Result<()> {
	// 1. Load current configs
	let config_path = getconf::get_config_dir().join(PLUGINS_CONFIG_FILE);
	let content = fs::read_to_string(&config_path).unwrap_or_else(|_| "{}".to_string());

	// Handle empty file case
	let effective_content = if content.trim().is_empty() {
		"{}"
	} else {
		&content
	};

	let mut configs: HashMap<String, ExternalPluginConfig> =
		serde_json::from_str(effective_content).unwrap_or_default();

	if !configs.contains_key(name) {
		return Err(anyhow!("Plugin '{}' not found.", name));
	}

	// 2. Remove from Persistence
	configs.remove(name);
	save_to_disk(&configs)?;

	// 3. Update Runtime Registry
	let registry_map = DashMap::new();
	for (name, cfg) in configs {
		registry_map.insert(name, Arc::new(ExternalPlugin::new(cfg)) as Arc<dyn Plugin>);
	}
	registry::load_external_plugins(registry_map);

	log(
		LogLevel::Info,
		&format!("➜ External plugin deleted: {}", name),
	);
	Ok(())
}
