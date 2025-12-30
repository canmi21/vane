/* src/modules/stack/protocol/application/hotswap.rs */

use super::model::{APPLICATION_REGISTRY, ApplicationConfig, SUPPORTED_APP_PROTOCOLS};
use crate::common::{
	getconf,
	hotswap::watch_loop,
	loader::{self, LoadResult},
};
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::sync::mpsc;

// Implement PreProcess for ApplicationConfig (no-op)
// Removed: Already implemented in model.rs

/// Scans the 'application' config subdirectory and rebuilds the registry.
pub fn scan_application_config(
	current_state: &DashMap<String, Arc<ApplicationConfig>>,
) -> DashMap<String, Arc<ApplicationConfig>> {
	let app_dir = getconf::get_config_dir().join("application");
	let new_registry = DashMap::new();

	if !app_dir.exists() || !app_dir.is_dir() {
		return new_registry;
	}

	for &protocol in SUPPORTED_APP_PROTOCOLS {
		let config_path = app_dir.join(protocol);
		let config: LoadResult<ApplicationConfig> = loader::load_config(protocol, &config_path);

		match config {
			LoadResult::Ok(config) => {
				new_registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded application config for protocol: {}", protocol),
				);
			}
			LoadResult::Invalid => {
				// KLKG: If load failed but we have an old one, reuse it.
				if let Some(old_config) = current_state.get(protocol) {
					log(
						LogLevel::Warn,
						&format!(
							"⚠ Failed to load new config for '{}'. Keeping last known good version.",
							protocol
						),
					);
					new_registry.insert(protocol.to_string(), old_config.value().clone());
				}
			}
			LoadResult::NotFound => {
				log(
					LogLevel::Info,
					&format!(
						"↓ Application protocol '{}' config removed or missing.",
						protocol
					),
				);
			}
		}
	}

	new_registry
}

/// Listens for update signals and reloads the application registry.
pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	watch_loop(rx, "Application", || async {
		let current_state = APPLICATION_REGISTRY.load();
		let new_registry = scan_application_config(&current_state);
		APPLICATION_REGISTRY.store(Arc::new(new_registry));
		log(LogLevel::Info, "✓ Application configurations synchronized.");
	})
	.await;
}
