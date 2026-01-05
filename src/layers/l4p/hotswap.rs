/* src/layers/l4p/hotswap.rs */

use super::model::{RESOLVER_REGISTRY, ResolverConfig, SUPPORTED_UPGRADE_PROTOCOLS};
use crate::common::{
	config::{file_loader, loader::{self, LoadResult}},
	sys::hotswap::watch_loop,
};
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::mpsc;

pub async fn scan_resolver_config(
	current_state: &DashMap<String, Arc<ResolverConfig>>,
) -> DashMap<String, Arc<ResolverConfig>> {
	let resolver_dir = file_loader::get_config_dir().join("resolver");
	let new_registry = DashMap::new();

	if let Ok(metadata) = fs::metadata(&resolver_dir).await {
		if !metadata.is_dir() {
			return new_registry;
		}
	} else {
		return new_registry;
	}

	for &protocol in SUPPORTED_UPGRADE_PROTOCOLS {
		let config_path = resolver_dir.join(protocol);
		let config: LoadResult<ResolverConfig> = loader::load_config(protocol, &config_path).await;

		match config {
			LoadResult::Ok(config) => {
				new_registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded resolver config: {}", protocol),
				);
			}
			LoadResult::Invalid => {
				if let Some(old_config) = current_state.get(protocol) {
					log(
						LogLevel::Warn,
						&format!("⚠ Keeping old config for {}", protocol),
					);
					new_registry.insert(protocol.to_string(), old_config.value().clone());
				}
			}
			LoadResult::NotFound => {
				log(
					LogLevel::Info,
					&format!("↓ Resolver protocol '{}' removed.", protocol),
				);
			}
		}
	}
	new_registry
}

pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	watch_loop(rx, "Resolver", || async {
		let current_state = RESOLVER_REGISTRY.load();
		let new_registry = scan_resolver_config(&current_state).await;
		RESOLVER_REGISTRY.store(Arc::new(new_registry));
		log(LogLevel::Info, "✓ Resolver configurations synchronized.");
	})
	.await;
}
