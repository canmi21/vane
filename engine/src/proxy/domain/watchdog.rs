/* engine/src/proxy/domain/watchdog.rs */

use super::hotswap;
use crate::daemon::config;
use fancy_log::{LogLevel, log};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Spawns a background task to watch for changes to domain directories.
pub fn start_domain_watchdog() {
	let config_dir = config::get_config_dir();
	tokio::spawn(async move {
		let (tx, mut rx) = tokio::sync::mpsc::channel(32);

		let mut watcher = match RecommendedWatcher::new(
			move |res| {
				if let Ok(event) = res {
					tx.blocking_send(event).expect("Failed to send fs event");
				}
			},
			notify::Config::default(),
		) {
			Ok(w) => w,
			Err(e) => {
				log(
					LogLevel::Error,
					&format!("Failed to create domain watchdog: {}", e),
				);
				return;
			}
		};

		if let Err(e) = watcher.watch(&config_dir, RecursiveMode::NonRecursive) {
			log(
				LogLevel::Error,
				&format!("Failed to start domain watchdog on {:?}: {}", config_dir, e),
			);
			return;
		}
		log(
			LogLevel::Info,
			&format!("Domain watchdog started on: {:?}", config_dir),
		);

		while let Some(event) = rx.recv().await {
			if matches!(event.kind, EventKind::Create(_) | EventKind::Remove(_)) {
				hotswap::reload_domain_list().await;
			}
		}
	});
}

/// Performs the initial scan and load of the domain list on startup.
pub async fn initial_load_domains() {
	log(LogLevel::Info, "Performing initial load of domain list...");
	hotswap::reload_domain_list().await;
}
