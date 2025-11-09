/* src/common/requirements.rs */

use crate::common::getconf;
use crate::modules::stack::transport::{health, session};
use fancy_log::{LogLevel, log};
use notify::{RecursiveMode, Watcher};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;

/// Ensures that all required directories and default files exist.
fn ensure_config_files_exist() {
	getconf::init_config_dir("listener");
	getconf::init_config_files(vec!["listener/unixsocket.yml"]);
}

/// Spawns a background task to watch the config directory with debouncing.
fn start_config_watcher() -> mpsc::Receiver<()> {
	let (debounced_tx, debounced_rx) = mpsc::channel(1);
	let listener_dir = getconf::get_config_dir().join("listener");

	tokio::spawn(async move {
		log(LogLevel::Debug, "➜ Starting config file watcher...");
		let (watcher_tx, mut watcher_rx) = mpsc::channel(32);
		let mut watcher =
			match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
				if let Ok(event) = res {
					let is_listener_related = event
						.paths
						.iter()
						.any(|path| path.starts_with(&listener_dir));

					if is_listener_related {
						log(
							LogLevel::Debug,
							&format!("⇆ FS Event in 'listener' detected: {:?}", event.kind),
						);
						let _ = watcher_tx.try_send(());
					} else {
						log(
							LogLevel::Debug,
							&format!("⚙ FS Event ignored (not in 'listener'): {:?}", event.paths),
						);
					}
				}
			}) {
				Ok(w) => w,
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Failed to create file watcher: {}", e),
					);
					return;
				}
			};

		let config_dir = getconf::get_config_dir();
		if let Err(e) = watcher.watch(&config_dir, RecursiveMode::Recursive) {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to watch config dir {}: {}",
					config_dir.display(),
					e
				),
			);
			return;
		}

		loop {
			if watcher_rx.recv().await.is_none() {
				break;
			}
			'debounce: loop {
				tokio::select! {
					Some(_) = watcher_rx.recv() => { continue 'debounce; }
					_ = sleep(Duration::from_secs(2)) => { if debounced_tx.send(()).await.is_err() { return; } break 'debounce; }
				}
			}
		}
	});
	debounced_rx
}

/// Runs all pre-flight checks and starts background tasks required by the application.
///
/// This function is the main entry point for application initialization. It ensures
/// the configuration structure is in place, starts the file watcher for dynamic
/// configuration updates, and launches periodic background tasks for health
/// checking and session management.
pub async fn initialize() -> mpsc::Receiver<()> {
	ensure_config_files_exist();
	let config_change_receiver = start_config_watcher();
	health::initial_health_check().await;
	health::start_periodic_health_checkers();
	session::start_session_cleanup_task();
	config_change_receiver
}
