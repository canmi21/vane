/* src/modules/stack/protocol/carrier/hotswap.rs */

use super::model::{RESOLVER_REGISTRY, ResolverConfig, SUPPORTED_UPGRADE_PROTOCOLS};
use crate::common::{
	getconf,
	hotswap::watch_loop,
	loader::{self, LoadResult},
};
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Scans the 'resolver' config subdirectory and rebuilds the registry.
pub fn scan_resolver_config(
	current_state: &DashMap<String, Arc<ResolverConfig>>,
) -> DashMap<String, Arc<ResolverConfig>> {
	let resolver_dir = getconf::get_config_dir().join("resolver");
	let new_registry = DashMap::new();

	if !resolver_dir.exists() || !resolver_dir.is_dir() {
		return new_registry;
	}

	for &protocol in SUPPORTED_UPGRADE_PROTOCOLS {
		let config_path = resolver_dir.join(protocol);
		let config: LoadResult<ResolverConfig> = loader::load_config(protocol, &config_path);

		match config {
			LoadResult::Ok(config) => {
				new_registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded resolver config for protocol: {}", protocol),
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
						"↓ Resolver protocol '{}' config removed or missing.",
						protocol
					),
				);
			}
		}
	}

	new_registry
}

/// Listens for update signals and reloads the resolver registry.
pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	watch_loop(rx, "Resolver", || async {
		let current_state = RESOLVER_REGISTRY.load();
		let new_registry = scan_resolver_config(&current_state);
		RESOLVER_REGISTRY.store(Arc::new(new_registry));
		log(LogLevel::Info, "✓ Resolver configurations synchronized.");
	})
	.await;
}
