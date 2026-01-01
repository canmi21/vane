/* src/modules/nodes/hotswap.rs */

use super::model::{NODES_STATE, NodesConfig};
use crate::common::{getconf, hotswap::watch_loop, loader};
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::sync::mpsc;

// Implement PreProcess for NodesConfig (no-op)
// Removed: Already implemented in model.rs

/// Scans and loads the nodes configuration.
pub async fn scan_nodes_config() -> Option<NodesConfig> {
	let config_dir = getconf::get_config_dir();
	let res: loader::LoadResult<NodesConfig> =
		loader::load_config("nodes", &config_dir.join("nodes")).await;

	match res {
		loader::LoadResult::Ok(config) => {
			log(LogLevel::Debug, "⚙ Loaded nodes configuration.");
			Some(config)
		}
		loader::LoadResult::NotFound => {
			log(
				LogLevel::Debug,
				"⚙ Nodes configuration file not found. Using default.",
			);
			Some(NodesConfig::default())
		}
		loader::LoadResult::Invalid => {
			// Returns None to signal the caller (listen_for_updates) to keep the old state
			None
		}
	}
}

/// Listens for update signals and reloads the nodes configuration.
pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	watch_loop(rx, "Nodes", || async {
		if let Some(new_config) = scan_nodes_config().await {
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
	})
	.await;
}
