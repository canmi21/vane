/* src/common/lifecycle.rs */

use crate::common::getconf;
use crate::modules::stack::carrier::quic::session as quic_session;
use crate::modules::stack::transport::{health, session};

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

/// Ensures all mandatory configuration directories and files exist.
pub async fn ensure_config_files_exist() {
	getconf::init_config_dirs(vec!["listener", "resolver", "certs", "application", "bin"]).await;
	getconf::init_config_files(vec!["listener/unixsocket.yml", "nodes.yml", "plugins.json"]).await;
}

/// Spawns essential background maintenance tasks.
pub async fn start_background_tasks() {
	health::initial_health_check().await;
	health::start_periodic_health_checkers();
	session::start_session_cleanup_task();
	quic_session::start_cleanup_task();
}
