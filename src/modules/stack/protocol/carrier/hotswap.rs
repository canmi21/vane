/* src/modules/stack/protocol/carrier/hotswap.rs */

use super::model::{RESOLVER_REGISTRY, ResolverConfig, SUPPORTED_UPGRADE_PROTOCOLS};
use crate::common::{getconf, hotswap::watch_loop, loader};
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Scans the 'resolver' config subdirectory and rebuilds the registry.
pub fn scan_resolver_config() -> DashMap<String, Arc<ResolverConfig>> {
	let resolver_dir = getconf::get_config_dir().join("resolver");
	let new_registry = DashMap::new();

	// Snapshot current state for fallback logic
	let current_state = RESOLVER_REGISTRY.load();

	if !resolver_dir.exists() || !resolver_dir.is_dir() {
		return new_registry;
	}

	for &protocol in SUPPORTED_UPGRADE_PROTOCOLS {
		let config_path = resolver_dir.join(protocol);
		let config: Option<ResolverConfig> = loader::load_config(protocol, &config_path);

		match config {
			Some(config) => {
				new_registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded resolver config for protocol: {}", protocol),
				);
			}
			None => {
				// Check if files exist to determine if it's a failure or removal
				let has_files = ["yaml", "yml", "json", "toml"]
					.iter()
					.any(|ext| config_path.with_extension(ext).exists());

				if has_files {
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
				} else {
					log(
						LogLevel::Info,
						&format!(
							"↓ Resolver protocol '{}' config removed. Disabling.",
							protocol
						),
					);
				}
			}
		}
	}

	new_registry
}

/// Listens for update signals and reloads the resolver registry.
pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	watch_loop(rx, "Resolver", || async {
		let new_registry = scan_resolver_config();
		RESOLVER_REGISTRY.store(Arc::new(new_registry));
		log(LogLevel::Info, "✓ Resolver configurations synchronized.");
	})
	.await;
}
