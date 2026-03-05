/* src/common/sys/lifecycle.rs */

use crate::common::config::file_loader;
use once_cell::sync::Lazy;
use std::time::Instant;

pub static START_TIME: Lazy<Instant> = Lazy::new(Instant::now);

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
	file_loader::init_config_dirs(vec!["listener", "resolver", "certs", "application", "bin"]).await;
	file_loader::init_config_files(vec!["listener/unixsocket.yml", "nodes.yml", "plugins.json"])
		.await;
}
