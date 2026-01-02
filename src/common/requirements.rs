/* src/common/requirements.rs */

use crate::common::getconf;
use crate::modules::stack::carrier::quic::session as quic_session;
use crate::modules::stack::transport::{health, session};
use fancy_log::{LogLevel, log};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::{ffi::OsStr, time::Duration};
use tokio::fs;
use tokio::sync::mpsc;
use tokio::time::sleep;

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

pub struct ConfigChangeReceivers {
	pub ports: mpsc::Receiver<()>,
	pub nodes: mpsc::Receiver<()>,
	pub resolvers: mpsc::Receiver<()>,
	pub certs: mpsc::Receiver<()>,
	pub applications: mpsc::Receiver<()>,
}

pub async fn ensure_config_files_exist() {
	getconf::init_config_dirs(vec!["listener", "resolver", "certs", "application", "bin"]).await;
	getconf::init_config_files(vec!["listener/unixsocket.yml", "nodes.yml", "plugins.json"]).await;
}

pub fn start_config_watchers_only() -> ConfigChangeReceivers {
	let (p_tx, p_rx) = mpsc::channel(1);
	let (n_tx, n_rx) = mpsc::channel(1);
	let (r_tx, r_rx) = mpsc::channel(1);
	let (c_tx, c_rx) = mpsc::channel(1);
	let (a_tx, a_rx) = mpsc::channel(1);
	let (pr_tx, mut pr_rx) = mpsc::channel(32);
	let (nr_tx, mut nr_rx) = mpsc::channel(32);
	let (rr_tx, mut rr_rx) = mpsc::channel(32);
	let (cr_tx, mut cr_rx) = mpsc::channel(32);
	let (ar_tx, mut ar_rx) = mpsc::channel(32);

	macro_rules! spawn_debouncer {
		($rx:ident, $tx:expr) => {
			tokio::spawn(async move {
				while $rx.recv().await.is_some() {
					loop {
						tokio::select! {
							Some(_) = $rx.recv() => continue,
							_ = sleep(Duration::from_secs(2)) => { let _ = $tx.send(()).await; break; }
						}
					}
				}
			});
		};
	}
	spawn_debouncer!(pr_rx, p_tx);
	spawn_debouncer!(nr_rx, n_tx);
	spawn_debouncer!(rr_rx, r_tx);
	spawn_debouncer!(cr_rx, c_tx);
	spawn_debouncer!(ar_rx, a_tx);

	tokio::spawn(async move {
		let (event_tx, mut event_rx) = mpsc::channel::<Event>(32);
		let mut watcher = match notify::recommended_watcher(move |res| {
			if let Ok(e) = res {
				let _ = event_tx.try_send(e);
			}
		}) {
			Ok(w) => w,
			Err(e) => {
				log(
					LogLevel::Error,
					&format!("✗ Failed to initialize config watcher: {}", e),
				);
				return;
			}
		};
		let config_dir = getconf::get_config_dir();
		let _ = watcher.watch(&config_dir, RecursiveMode::Recursive);

		let l_dir = fs::canonicalize(config_dir.join("listener"))
			.await
			.unwrap_or(config_dir.join("listener"));
		let r_dir = fs::canonicalize(config_dir.join("resolver"))
			.await
			.unwrap_or(config_dir.join("resolver"));
		let c_dir = fs::canonicalize(config_dir.join("certs"))
			.await
			.unwrap_or(config_dir.join("certs"));
		let a_dir = fs::canonicalize(config_dir.join("application"))
			.await
			.unwrap_or(config_dir.join("application"));

		while let Some(event) = event_rx.recv().await {
			match event.kind {
				EventKind::Access(_) | EventKind::Other => continue,
				_ => {}
			}
			if event.paths.iter().any(|p| p.starts_with(&l_dir)) {
				let _ = pr_tx.try_send(());
			} else if event
				.paths
				.iter()
				.any(|p| p.file_stem() == Some(OsStr::new("nodes")))
			{
				let _ = nr_tx.try_send(());
			} else if event.paths.iter().any(|p| p.starts_with(&r_dir)) {
				let _ = rr_tx.try_send(());
			} else if event.paths.iter().any(|p| p.starts_with(&c_dir)) {
				let _ = cr_tx.try_send(());
			} else if event.paths.iter().any(|p| p.starts_with(&a_dir)) {
				let _ = ar_tx.try_send(());
			}
		}
	});

	ConfigChangeReceivers {
		ports: p_rx,
		nodes: n_rx,
		resolvers: r_rx,
		certs: c_rx,
		applications: a_rx,
	}
}

pub async fn start_background_tasks() {
	health::initial_health_check().await;
	health::start_periodic_health_checkers();
	session::start_session_cleanup_task();
	quic_session::start_cleanup_task();
}
