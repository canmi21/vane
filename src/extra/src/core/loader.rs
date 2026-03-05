/* src/plugins/core/loader.rs */

use crate::core::external::ExternalPlugin;
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use vane_engine::engine::interfaces::{ExternalPluginConfig, Plugin};
use vane_engine::registry;
use vane_primitives::common::config::file_loader;

const PLUGINS_CONFIG_FILE: &str = "plugins.json";

pub async fn initialize() -> usize {
	let config_path = file_loader::get_config_dir().join(PLUGINS_CONFIG_FILE);
	if fs::metadata(&config_path).await.is_err() {
		let empty: HashMap<String, ExternalPluginConfig> = HashMap::new();
		if let Ok(c) = serde_json::to_string_pretty(&empty) {
			let _ = fs::write(&config_path, c).await;
		}
		return 0;
	}
	let mut content = fs::read_to_string(&config_path).await.unwrap_or_default();
	if content.trim().is_empty() {
		content = "{}".to_owned();
		let _ = fs::write(&config_path, &content).await;
	}
	let configs: HashMap<String, ExternalPluginConfig> =
		serde_json::from_str(&content).unwrap_or_default();
	let registry_map = DashMap::new();
	let mut count = 0;
	for (name, config) in configs {
		registry_map.insert(
			name,
			Arc::new(ExternalPlugin::new(config)) as Arc<dyn Plugin>,
		);
		count += 1;
	}
	registry::load_external_plugins(registry_map);
	if count > 0 {
		log(
			LogLevel::Info,
			&format!("✓ Loaded {count} external plugins."),
		);
		start_background_health_check();
	}
	count
}

fn start_background_health_check() {
	tokio::spawn(async move {
		let mins = envflag::get::<u64>("EXTERNAL_PLUGIN_CHECK_INTERVAL_MINS", 15);
		let mut interval = tokio::time::interval(Duration::from_secs(mins * 60));
		loop {
			interval.tick().await;
			for plugin in registry::list_external_plugins() {
				let name = plugin.name().to_owned();
				if let Some(ext) = plugin.as_any().downcast_ref::<ExternalPlugin>() {
					let res = ext.validate_connectivity().await;
					registry::EXTERNAL_PLUGIN_STATUS.insert(name, res.map_err(|e| e.to_string()));
				}
			}
		}
	});
}

async fn save_to_disk(configs: &HashMap<String, ExternalPluginConfig>) -> Result<()> {
	let path = file_loader::get_config_dir().join(PLUGINS_CONFIG_FILE);
	fs::write(path, serde_json::to_string_pretty(configs)?).await?;
	Ok(())
}

pub async fn register_plugin(config: ExternalPluginConfig) -> Result<()> {
	if registry::get_internal_plugin(&config.name).is_some() {
		return Err(anyhow!("Conflict with built-in."));
	}
	let plugin = ExternalPlugin::new(config.clone());
	plugin.validate_connectivity().await?;
	let path = file_loader::get_config_dir().join(PLUGINS_CONFIG_FILE);
	let content = fs::read_to_string(&path)
		.await
		.unwrap_or_else(|_| "{}".to_owned());
	let mut configs: HashMap<String, ExternalPluginConfig> =
		serde_json::from_str(&content).unwrap_or_default();
	configs.insert(config.name.clone(), config);
	save_to_disk(&configs).await?;
	initialize().await;
	Ok(())
}

pub async fn delete_plugin(name: &str) -> Result<()> {
	let path = file_loader::get_config_dir().join(PLUGINS_CONFIG_FILE);
	let content = fs::read_to_string(&path)
		.await
		.unwrap_or_else(|_| "{}".to_owned());
	let mut configs: HashMap<String, ExternalPluginConfig> =
		serde_json::from_str(&content).unwrap_or_default();
	if configs.remove(name).is_none() {
		return Err(anyhow!("Not found."));
	}
	save_to_disk(&configs).await?;
	initialize().await;
	Ok(())
}
