/* src/modules/stack/protocol/application/hotswap.rs */

use super::model::{APPLICATION_REGISTRY, ApplicationConfig, SUPPORTED_APP_PROTOCOLS};
use crate::common::getconf;
use crate::modules::stack::transport::loader;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Scans the 'application' config subdirectory and rebuilds the registry.
///
/// Strategy:
/// - **Success**: Updates registry with new config.
/// - **Conflict/Error**: Falls back to the currently loaded config for that protocol.
/// - **Missing**: Disables/Removes the protocol.
pub fn scan_application_config() -> DashMap<String, Arc<ApplicationConfig>> {
	let app_dir = getconf::get_config_dir().join("application");
	let new_registry = DashMap::new();

	// Snapshot current state for fallback
	let current_state = APPLICATION_REGISTRY.load();

	if !app_dir.exists() || !app_dir.is_dir() {
		return new_registry;
	}

	for &protocol in SUPPORTED_APP_PROTOCOLS {
		let mut found_files = Vec::new();
		let extensions = ["yaml", "yml", "json", "toml"];

		// 1. Scan for all possible config files
		for ext in &extensions {
			let file_path = app_dir.join(format!("{}.{}", protocol, ext));
			if file_path.exists() {
				found_files.push(file_path);
			}
		}

		// 2. Decide action based on file count
		if found_files.is_empty() {
			if current_state.contains_key(protocol) {
				log(
					LogLevel::Info,
					&format!(
						"↓ Application protocol '{}' config removed. Disabling.",
						protocol
					),
				);
			}
			continue;
		}

		if found_files.len() > 1 {
			let file_names: Vec<_> = found_files
				.iter()
				.filter_map(|p| p.file_name().and_then(|n| n.to_str()))
				.collect();

			log(
				LogLevel::Error,
				&format!(
					"✗ Config Conflict for '{}': Multiple files found {:?}. Keeping last known good version.",
					protocol, file_names
				),
			);

			if let Some(old_config) = current_state.get(protocol) {
				new_registry.insert(protocol.to_string(), old_config.value().clone());
			}
			continue;
		}

		// Case: Single File found
		let file_path = &found_files[0];
		match loader::load_file::<ApplicationConfig>(file_path) {
			Some(config) => {
				new_registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded application config for protocol: {}", protocol),
				);
			}
			None => {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Failed to load new config for '{}'. Keeping last known good version.",
						protocol
					),
				);

				if let Some(old_config) = current_state.get(protocol) {
					new_registry.insert(protocol.to_string(), old_config.value().clone());
				}
			}
		}
	}

	new_registry
}

/// Listens for update signals and reloads the application registry.
pub async fn listen_for_updates(mut rx: mpsc::Receiver<()>) {
	while rx.recv().await.is_some() {
		log(
			LogLevel::Info,
			"➜ Application config change detected, resyncing...",
		);

		let new_registry = scan_application_config();

		// Atomic Swap
		APPLICATION_REGISTRY.store(Arc::new(new_registry));

		log(LogLevel::Info, "✓ Application configurations synchronized.");
	}
}
