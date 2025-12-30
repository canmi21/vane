/* src/modules/stack/protocol/application/hotswap.rs */

use super::model::{APPLICATION_REGISTRY, ApplicationConfig, SUPPORTED_APP_PROTOCOLS};
use crate::common::{getconf, hotswap::watch_loop, loader};
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::sync::mpsc;

// Implement PreProcess for ApplicationConfig (no-op)
// Removed: Already implemented in model.rs

/// Scans the 'application' config subdirectory and rebuilds the registry.
pub fn scan_application_config() -> DashMap<String, Arc<ApplicationConfig>> {
	let app_dir = getconf::get_config_dir().join("application");
	let new_registry = DashMap::new();

	// Snapshot current state for fallback
	let current_state = APPLICATION_REGISTRY.load();

	if !app_dir.exists() || !app_dir.is_dir() {
		return new_registry;
	}

	for &protocol in SUPPORTED_APP_PROTOCOLS {
		let config_path = app_dir.join(protocol);
		let config: Option<ApplicationConfig> = loader::load_config(protocol, &config_path);

		match config {
			Some(config) => {
				new_registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded application config for protocol: {}", protocol),
				);
			}
			None => {
				// If load failed or no file found
				if current_state.contains_key(protocol) {
					// Check if it was a failure or just removal
					// load_config logs errors, so we assume if it returns None it might be missing or invalid
					// But load_config returns None for BOTH missing AND invalid.
					// We need to know if we should fallback or disable.
					//
					// Strategy: If any file exists but load failed -> Fallback.
					// If no file exists -> Disable.
					//
					// Since `load_config` abstracts this check, we rely on its internal logging for errors.
					// For "Keep Last Known Good", we need to check existence manually or modify loader.
					//
					// REVISION: The common loader doesn't distinguish "missing" from "invalid".
					// To strictly implement K-L-K-G, we need to check if files exist.
					let has_files = ["yaml", "yml", "json", "toml"]
						.iter()
						.any(|ext| config_path.with_extension(ext).exists());

					if has_files {
						log(
							LogLevel::Warn,
							&format!(
								"⚠ Config exists but failed to load for '{}'. Keeping last known good version.",
								protocol
							),
						);
						if let Some(old_config) = current_state.get(protocol) {
							new_registry.insert(protocol.to_string(), old_config.value().clone());
						}
					} else {
						log(
							LogLevel::Info,
							&format!(
								"↓ Application protocol '{}' config removed. Disabling.",
								protocol
							),
						);
					}
				}
			}
		}
	}

	new_registry
}

/// Listens for update signals and reloads the application registry.
pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	watch_loop(rx, "Application", || async {
		let new_registry = scan_application_config();
		APPLICATION_REGISTRY.store(Arc::new(new_registry));
		log(LogLevel::Info, "✓ Application configurations synchronized.");
	})
	.await;
}
