/* src/common/requirements.rs */

use crate::common::getconf;
use crate::modules::stack::transport::{health, session};
use fancy_log::{LogLevel, log};
use notify::{Event, RecursiveMode, Watcher};
use std::{ffi::OsStr, fs, time::Duration};
use tokio::sync::mpsc;
use tokio::time::sleep;

// --- Error Handling Definitions ---

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("IO Error: {0}")]
	Io(String),
	#[error("TLS Error: {0}")]
	Tls(String),
	#[error("Configuration Error: {0}")]
	Configuration(String),
	#[error("System Error: {0}")]
	System(String),
	#[error("Not Implemented: {0}")]
	NotImplemented(String),
	#[error("Anyhow: {0}")]
	Anyhow(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

// --- Config Watcher Logic ---

/// A container for the different configuration change receivers.
pub struct ConfigChangeReceivers {
	pub ports: mpsc::Receiver<()>,
	pub nodes: mpsc::Receiver<()>,
	pub resolvers: mpsc::Receiver<()>,
	pub certs: mpsc::Receiver<()>,
	pub applications: mpsc::Receiver<()>,
}

/// Ensures that all required directories and default files exist.
fn ensure_config_files_exist() {
	getconf::init_config_dirs(vec!["listener", "resolver", "certs", "application"]);
	getconf::init_config_files(vec!["listener/unixsocket.yml", "nodes.yml", "plugins.json"]);
}

/// Spawns background tasks to watch the config directory and notify modules of changes.
fn start_config_watchers() -> ConfigChangeReceivers {
	let (ports_debounced_tx, ports_debounced_rx) = mpsc::channel(1);
	let (nodes_debounced_tx, nodes_debounced_rx) = mpsc::channel(1);
	let (resolvers_debounced_tx, resolvers_debounced_rx) = mpsc::channel(1);
	let (certs_debounced_tx, certs_debounced_rx) = mpsc::channel(1);
	let (apps_debounced_tx, apps_debounced_rx) = mpsc::channel(1);

	// Create raw channels to send immediate, pre-debounced signals.
	let (ports_raw_tx, ports_raw_rx) = mpsc::channel(32);
	let (nodes_raw_tx, nodes_raw_rx) = mpsc::channel(32);
	let (resolvers_raw_tx, resolvers_raw_rx) = mpsc::channel(32);
	let (certs_raw_tx, certs_raw_rx) = mpsc::channel(32);
	let (apps_raw_tx, apps_raw_rx) = mpsc::channel(32);

	// Helper macro to spawn debouncers
	// We explicitly re-bind `rx` as mutable inside the async block to satisfy `recv(&mut self)`
	macro_rules! spawn_debouncer {
		($raw_rx:expr, $debounced_tx:expr, $name:expr) => {
			tokio::spawn(async move {
				let mut rx = $raw_rx;
				while rx.recv().await.is_some() {
					'debounce: loop {
						tokio::select! {
								Some(_) = rx.recv() => { continue 'debounce; }
								_ = sleep(Duration::from_secs(2)) => {
										if $debounced_tx.send(()).await.is_err() { return; }
										break 'debounce;
								}
						}
					}
				}
			});
		};
	}

	spawn_debouncer!(ports_raw_rx, ports_debounced_tx, "ports");
	spawn_debouncer!(nodes_raw_rx, nodes_debounced_tx, "nodes");
	spawn_debouncer!(resolvers_raw_rx, resolvers_debounced_tx, "resolvers");
	spawn_debouncer!(certs_raw_rx, certs_debounced_tx, "certs");
	// FIXED: Passed apps_raw_rx (Receiver) instead of apps_raw_tx (Sender)
	spawn_debouncer!(apps_raw_rx, apps_debounced_tx, "applications");

	// Spawn the main, long-running watcher task.
	tokio::spawn(async move {
		log(LogLevel::Debug, "➜ Starting config file watcher...");
		let (event_tx, mut event_rx) = mpsc::channel::<Event>(32);

		let mut watcher =
			match notify::recommended_watcher(move |res: std::result::Result<Event, notify::Error>| {
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

		let join_canon = |sub: &str| -> std::path::PathBuf {
			fs::canonicalize(config_dir.join(sub)).unwrap_or_else(|_| config_dir.join(sub))
		};

		let listener_dir = join_canon("listener");
		let resolver_dir = join_canon("resolver");
		let certs_dir = join_canon("certs");
		let app_dir = join_canon("application");

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
			} else if event.paths.iter().any(|p| p.starts_with(&resolver_dir)) {
				let _ = resolvers_raw_tx.try_send(());
			} else if event.paths.iter().any(|p| p.starts_with(&certs_dir)) {
				let _ = certs_raw_tx.try_send(());
			} else if event.paths.iter().any(|p| p.starts_with(&app_dir)) {
				let _ = apps_raw_tx.try_send(());
			}
		}
	});

	ConfigChangeReceivers {
		ports: ports_debounced_rx,
		nodes: nodes_debounced_rx,
		resolvers: resolvers_debounced_rx,
		certs: certs_debounced_rx,
		applications: apps_debounced_rx,
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
