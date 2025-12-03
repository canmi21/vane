/* src/common/requirements.rs */

use crate::common::getconf;
use crate::modules::stack::transport::{health, session};
use fancy_log::{LogLevel, log};
use notify::{Event, RecursiveMode, Watcher};
use std::{ffi::OsStr, fs, time::Duration};
use tokio::sync::mpsc;
use tokio::time::sleep;

/// A container for the different configuration change receivers.
pub struct ConfigChangeReceivers {
	pub ports: mpsc::Receiver<()>,
	pub nodes: mpsc::Receiver<()>,
}

/// Ensures that all required directories and default files exist.
fn ensure_config_files_exist() {
	getconf::init_config_dirs(vec!["listener", "resolver"]);
	getconf::init_config_files(vec!["listener/unixsocket.yml", "nodes.yml", "plugins.json"]);
}

/// Spawns background tasks to watch the config directory and notify modules of changes.
fn start_config_watchers() -> ConfigChangeReceivers {
	let (ports_debounced_tx, ports_debounced_rx) = mpsc::channel(1);
	let (nodes_debounced_tx, nodes_debounced_rx) = mpsc::channel(1);

	// Create raw channels to send immediate, pre-debounced signals.
	let (ports_raw_tx, mut ports_raw_rx) = mpsc::channel(32);
	let (nodes_raw_tx, mut nodes_raw_rx) = mpsc::channel(32);

	// Spawn a dedicated, long-running task for the ports debouncer.
	tokio::spawn(async move {
		while ports_raw_rx.recv().await.is_some() {
			'debounce: loop {
				tokio::select! {
						Some(_) = ports_raw_rx.recv() => { continue 'debounce; }
						_ = sleep(Duration::from_secs(2)) => {
								if ports_debounced_tx.send(()).await.is_err() { return; }
								break 'debounce;
						}
				}
			}
		}
	});

	// Spawn a dedicated, long-running task for the nodes debouncer.
	tokio::spawn(async move {
		while nodes_raw_rx.recv().await.is_some() {
			'debounce: loop {
				tokio::select! {
						Some(_) = nodes_raw_rx.recv() => { continue 'debounce; }
						_ = sleep(Duration::from_secs(2)) => {
								if nodes_debounced_tx.send(()).await.is_err() { return; }
								break 'debounce;
						}
				}
			}
		}
	});

	// Spawn the main, long-running watcher task. Its primary job is to keep the
	// `watcher` object alive and multiplex events to the debouncer tasks.
	tokio::spawn(async move {
		log(LogLevel::Debug, "➜ Starting config file watcher...");
		let (event_tx, mut event_rx) = mpsc::channel::<Event>(32);

		// This `watcher` is moved into the async block and will live as long as the task.
		let mut watcher = match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
			if let Ok(event) = res {
				let _ = event_tx.try_send(event);
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

		// Canonicalize the listener directory path to resolve symlinks (e.g., /var -> /private/var on macOS).
		// This ensures that the path we are checking against matches the fully resolved path from the FS event.
		let listener_dir = match fs::canonicalize(config_dir.join("listener")) {
			Ok(path) => path,
			Err(e) => {
				log(
					LogLevel::Error,
					&format!(
						"✗ Could not canonicalize listener directory path. File watching may be unreliable: {}",
						e
					),
				);
				// Fallback to the non-canonicalized path, though the problem will likely persist.
				config_dir.join("listener")
			}
		};

		// This loop runs forever, keeping the task and the `watcher` alive.
		while let Some(event) = event_rx.recv().await {
			log(
				LogLevel::Debug,
				&format!("⇆ FS Event detected: {:?}", event.kind),
			);
			if event.paths.iter().any(|p| p.starts_with(&listener_dir)) {
				let _ = ports_raw_tx.try_send(());
			} else if event
				.paths
				.iter()
				.any(|p| p.file_stem() == Some(OsStr::new("nodes")))
			{
				let _ = nodes_raw_tx.try_send(());
			} else {
				log(
					LogLevel::Debug,
					&format!("⚙ FS Event ignored (unrelated path): {:?}", event.paths),
				);
			}
		}
	});

	ConfigChangeReceivers {
		ports: ports_debounced_rx,
		nodes: nodes_debounced_rx,
	}
}

/// Runs all pre-flight checks and starts background tasks required by the application.
pub async fn initialize() -> ConfigChangeReceivers {
	ensure_config_files_exist();
	let config_change_receivers = start_config_watchers();
	health::initial_health_check().await;
	health::start_periodic_health_checkers();
	session::start_session_cleanup_task();
	config_change_receivers
}
