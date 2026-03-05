/* src/core/src/lazycert/mod.rs */

#[cfg(feature = "lazycert")]
pub mod client;
#[cfg(feature = "lazycert")]
pub mod sync;

#[cfg(feature = "lazycert")]
use client::LazyCertClient;
#[cfg(feature = "lazycert")]
use fancy_log::{LogLevel, log};
#[cfg(feature = "lazycert")]
use once_cell::sync::OnceCell;
#[cfg(feature = "lazycert")]
use std::sync::Arc;
#[cfg(feature = "lazycert")]
use tokio::sync::RwLock;
#[cfg(feature = "lazycert")]
use vane_engine::config::LazyCertConfig;

/// Global LazyCert configuration (supports hot-reload)
#[cfg(feature = "lazycert")]
pub static LAZYCERT_CONFIG: OnceCell<Arc<RwLock<Option<LazyCertConfig>>>> = OnceCell::new();

/// Global LazyCert client instance
#[cfg(feature = "lazycert")]
pub static LAZYCERT_CLIENT: OnceCell<Arc<RwLock<Option<Arc<LazyCertClient>>>>> = OnceCell::new();

/// Initialize LazyCert integration
pub async fn initialize() {
	#[cfg(feature = "lazycert")]
	{
		// Initialize global state
		let _ = LAZYCERT_CONFIG.set(Arc::new(RwLock::new(None)));
		let _ = LAZYCERT_CLIENT.set(Arc::new(RwLock::new(None)));

		// Initial setup from global config
		update_from_config().await;

		// Start watching for changes
		tokio::spawn(async {
			let config_manager = vane_engine::config::get();
			if let Some(lc) = &config_manager.lazycert {
				let mut rx = lc.subscribe();
				while let Ok(_event) = rx.recv().await {
					update_from_config().await;
				}
			}
		});
	}
}

/// Update internal state from global configuration
pub async fn update_from_config() {
	#[cfg(feature = "lazycert")]
	{
		let config_manager = vane_engine::config::get();
		let new_config = config_manager
			.lazycert
			.as_ref()
			.and_then(|lc| lc.get())
			.map(|arc| (*arc).clone());

		// Update global config
		if let Some(config_lock) = LAZYCERT_CONFIG.get() {
			let mut config = config_lock.write().await;
			*config = new_config.clone();
		}

		// Recreate client if config changed
		if let Some(cfg) = new_config {
			if !cfg.enabled {
				log(LogLevel::Info, "LazyCert integration disabled in config");
				stop_sync().await;
				return;
			}

			let client = Arc::new(LazyCertClient::new(&cfg.url, cfg.token.clone()));
			// Check connectivity
			match client.health().await {
				Ok(true) => {
					log(
						LogLevel::Info,
						&format!("Connected to LazyCert at {}", cfg.url),
					);
				}
				_ => {
					log(
						LogLevel::Warn,
						&format!("Cannot reach LazyCert at {}, will retry", cfg.url),
					);
				}
			}

			// Update global client
			if let Some(client_lock) = LAZYCERT_CLIENT.get() {
				let mut c = client_lock.write().await;
				*c = Some(client.clone());
			}

			// Start/restart background sync
			sync::spawn_sync_task(client, std::time::Duration::from_secs(cfg.poll_interval));

			log(
				LogLevel::Info,
				&format!(
					"LazyCert sync started (poll interval: {}s)",
					cfg.poll_interval
				),
			);
		} else {
			stop_sync().await;
		}
	}
}

/// Stop sync task and clear client
#[cfg(feature = "lazycert")]
async fn stop_sync() {
	if let Some(client_lock) = LAZYCERT_CLIENT.get() {
		let mut c = client_lock.write().await;
		*c = None;
	}
	// sync task will stop on next iteration when it sees no client
}

/// Get current public IP for certificate requests
#[cfg(feature = "lazycert")]
pub async fn get_public_ip() -> Option<String> {
	if let Some(config_lock) = LAZYCERT_CONFIG.get() {
		let config = config_lock.read().await;
		if let Some(cfg) = config.as_ref() {
			if cfg.public_ip == "auto" {
				// Auto-detect public IP
				return detect_public_ip().await;
			}
			return Some(cfg.public_ip.clone());
		}
	}
	None
}

/// Detect public IP via external service
#[cfg(feature = "lazycert")]
async fn detect_public_ip() -> Option<String> {
	// Use a simple IP detection service
	let client = reqwest::Client::new();
	if let Ok(resp) = client.get("https://api.ipify.org").send().await
		&& let Ok(ip) = resp.text().await
	{
		return Some(ip.trim().to_owned());
	}
	None
}
