/* src/common/requirements.rs */

use crate::common::getconf;
use fancy_log::{LogLevel, log};
use notify::{RecursiveMode, Watcher};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;

/// Ensures that all required directories and default files exist.
fn ensure_config_files_exist() {
	getconf::init_config_files(vec!["instance"]);
}

/// Spawns a background task to watch the config directory with debouncing.
/// Returns a receiver channel that will get a message on stable changes.
fn start_config_watcher() -> mpsc::Receiver<()> {
	// The final channel that the main application will listen on.
	let (debounced_tx, debounced_rx) = mpsc::channel(1);

	tokio::spawn(async move {
		log(LogLevel::Debug, "➜ Starting config file watcher...");

		// An internal channel for the watcher to send raw, non-debounced events.
		let (watcher_tx, mut watcher_rx) = mpsc::channel(32);

		let mut watcher =
			match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
				if let Ok(event) = res {
					log(
						LogLevel::Debug,
						&format!("⇆ FS Event detected: {:?}", event.kind),
					);
					// Use try_send to avoid blocking the file watcher thread.
					let _ = watcher_tx.try_send(());
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

		// This loop performs the debouncing.
		loop {
			// Wait for the first event. If the channel closes, the task ends.
			if watcher_rx.recv().await.is_none() {
				break;
			}

			// Wait for a 2-second quiet period.
			'debounce: loop {
				tokio::select! {
					// If another event comes in, restart the 2-second timer.
					_ = watcher_rx.recv() => {
						continue 'debounce;
					}
					// If the timer completes, the changes are stable.
					_ = sleep(Duration::from_secs(2)) => {
						// Send the single "stable" signal to the main application.
						if debounced_tx.send(()).await.is_err() {
							// If the main app is no longer listening, we can stop.
							return;
						}
						break 'debounce;
					}
				}
			}
		}
	});

	// Return the receiver for the debounced channel.
	debounced_rx
}

/// Runs all pre-flight checks and starts background tasks.
pub async fn initialize() -> mpsc::Receiver<()> {
	// Part 1: Pre-flight checks
	ensure_config_files_exist();

	// Part 2: Pre-flight tasks
	let config_change_receiver = start_config_watcher();

	config_change_receiver
}
