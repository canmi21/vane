/* src/modules/stack/protocol/carrier/hotswap.rs */

use super::model::{RESOLVER_REGISTRY, ResolverConfig, SUPPORTED_UPGRADE_PROTOCOLS};
use crate::common::getconf;
use crate::modules::stack::transport::loader;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Scans the 'resolver' config subdirectory and rebuilds the registry.
///
/// Implements "Keep Last Known Good" strategy:
/// - **Success**: Updates to the new configuration.
/// - **Failure** (Conflict/Invalid): Retains the configuration from the CURRENT registry.
///   (On first load, current is empty, so it correctly falls back to "disabled").
/// - **Missing**: Removes the configuration (disable).
pub fn scan_resolver_config() -> DashMap<String, Arc<ResolverConfig>> {
	let resolver_dir = getconf::get_config_dir().join("resolver");
	let new_registry = DashMap::new();

	// Snapshot current state for fallback logic
	let current_state = RESOLVER_REGISTRY.load();

	if !resolver_dir.exists() || !resolver_dir.is_dir() {
		return new_registry;
	}

	for &protocol in SUPPORTED_UPGRADE_PROTOCOLS {
		let mut found_files = Vec::new();
		let extensions = ["yaml", "yml", "json", "toml"];

		// 1. Scan for all possible config files
		for ext in &extensions {
			let file_path = resolver_dir.join(format!("{}.{}", protocol, ext));
			if file_path.exists() {
				found_files.push(file_path);
			}
		}

		// 2. Decide action based on file count
		if found_files.is_empty() {
			// Case: Removed
			if current_state.contains_key(protocol) {
				log(
					LogLevel::Info,
					&format!(
						"↓ Resolver protocol '{}' config file removed. Disabling.",
						protocol
					),
				);
			}
			continue;
		}

		if found_files.len() > 1 {
			// Case: Conflict
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

			// Fallback: Copy from old registry if exists
			if let Some(old_config) = current_state.get(protocol) {
				new_registry.insert(protocol.to_string(), old_config.value().clone());
			}
			continue;
		}

		// Case: Single File found, try to load
		let file_path = &found_files[0];
		match loader::load_file::<ResolverConfig>(file_path) {
			Some(config) => {
				// Success
				new_registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded resolver config for protocol: {}", protocol),
				);
			}
			None => {
				// Failure (Validation/Parse error) - Logged by loader
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Failed to load new config for '{}'. Keeping last known good version.",
						protocol
					),
				);

				// Fallback: Copy from old registry if exists
				if let Some(old_config) = current_state.get(protocol) {
					new_registry.insert(protocol.to_string(), old_config.value().clone());
				}
			}
		}
	}

	new_registry
}

/// Listens for update signals and reloads the resolver registry.
pub async fn listen_for_updates(mut rx: mpsc::Receiver<()>) {
	while rx.recv().await.is_some() {
		log(
			LogLevel::Info,
			"➜ Resolver config change detected, resyncing...",
		);

		let new_registry = scan_resolver_config();

		// Atomic Swap
		RESOLVER_REGISTRY.store(Arc::new(new_registry));

		log(LogLevel::Info, "✓ Resolver configurations synchronized.");
	}
}
