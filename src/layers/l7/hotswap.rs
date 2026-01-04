/* src/layers/l7/hotswap.rs */

use super::model::{APPLICATION_REGISTRY, ApplicationConfig, SUPPORTED_APP_PROTOCOLS};
use crate::common::{
	config::{
		getconf,
		loader::{self, LoadResult},
	},
	sys::hotswap::watch_loop,
};
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::mpsc;

pub async fn scan_application_config(
	current_state: &DashMap<String, Arc<ApplicationConfig>>,
) -> DashMap<String, Arc<ApplicationConfig>> {
	let app_dir = getconf::get_config_dir().join("application");
	let new_registry = DashMap::new();

	if let Ok(metadata) = fs::metadata(&app_dir).await {
		if !metadata.is_dir() {
			return new_registry;
		}
	} else {
		return new_registry;
	}

	for &protocol in SUPPORTED_APP_PROTOCOLS {
		let config_path = app_dir.join(protocol);
		let config: LoadResult<ApplicationConfig> = loader::load_config(protocol, &config_path).await;

		match config {
			LoadResult::Ok(config) => {
				new_registry.insert(protocol.to_string(), Arc::new(config));
				log(
					LogLevel::Debug,
					&format!("⚙ Loaded application config: {}", protocol),
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
					&format!("↓ Application protocol '{}' removed.", protocol),
				);
			}
		}
	}
	new_registry
}

pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	watch_loop(rx, "Application", || async {
		let current_state = APPLICATION_REGISTRY.load();
		let new_registry = scan_application_config(&current_state).await;
		APPLICATION_REGISTRY.store(Arc::new(new_registry));
		log(LogLevel::Info, "✓ Application configurations synchronized.");
	})
	.await;
}
