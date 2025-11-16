/* src/modules/nodes/hotswap.rs */

use super::model::{NODES_STATE, NodesConfig};
use crate::common::getconf;
use crate::modules::stack::transport::loader::PreProcess;
use fancy_log::{LogLevel, log};
use std::{fs, path::PathBuf, sync::Arc};
use tokio::sync::mpsc;
use validator::Validate;

/// Scans and loads the nodes configuration, handling conflicts and validation.
pub fn scan_nodes_config() -> Option<NodesConfig> {
	let config_dir = getconf::get_config_dir();
	// FIX: Add "yml" to the list of supported extensions for YAML files.
	let supported_extensions = ["yml", "yaml", "json", "toml"];
	let mut found_files: Vec<PathBuf> = Vec::new();

	for ext in supported_extensions {
		let file_path = config_dir.join("nodes").with_extension(ext);
		if file_path.exists() {
			found_files.push(file_path);
		}
	}

	if found_files.len() > 1 {
		log(
			LogLevel::Error,
			&format!(
				"✗ Conflicting configuration files found: {:?}. Only one 'nodes' file is allowed.",
				found_files
			),
		);
		return None;
	}

	if found_files.is_empty() {
		return Some(NodesConfig::default());
	}

	let config_path = &found_files[0];
	let content = match fs::read_to_string(config_path) {
		Ok(c) => c,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to read nodes config file {}: {}",
					config_path.display(),
					e
				),
			);
			return None;
		}
	};

	let extension = config_path
		.extension()
		.and_then(|s| s.to_str())
		.unwrap_or("");
	let parse_result: Result<NodesConfig, String> = match extension {
		"yml" | "yaml" => serde_yaml::from_str(&content).map_err(|e| e.to_string()),
		"json" => serde_json::from_str(&content).map_err(|e| e.to_string()),
		"toml" => toml::from_str(&content).map_err(|e| e.to_string()),
		_ => unreachable!(),
	};

	match parse_result {
		Ok(mut config) => {
			if let Err(e) = config.validate() {
				log(
					LogLevel::Error,
					&format!("✗ Nodes configuration validation failed: {}", e),
				);
				return None;
			}
			config.pre_process();
			Some(config)
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to parse nodes config file {}: {}",
					config_path.display(),
					e
				),
			);
			None
		}
	}
}

/// Listens for update signals and reloads the nodes configuration.
pub async fn listen_for_updates(mut rx: mpsc::Receiver<()>) {
	while rx.recv().await.is_some() {
		log(
			LogLevel::Info,
			"➜ Config change signal received, reloading nodes...",
		);

		if let Some(new_config) = scan_nodes_config() {
			let old_config = NODES_STATE.load();
			if old_config.nodes != new_config.nodes {
				NODES_STATE.store(Arc::new(new_config));
				log(
					LogLevel::Info,
					"✓ Nodes configuration updated successfully.",
				);
			} else {
				log(LogLevel::Debug, "⚙ No effective node changes detected.");
			}
		} else {
			log(
				LogLevel::Error,
				"✗ Failed to reload nodes configuration, keeping the old version.",
			);
		}
	}
}
