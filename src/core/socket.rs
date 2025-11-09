/* src/core/socket.rs */

use crate::common::getenv;
use fancy_log::{LogLevel, log};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::net::UnixListener;
use tokio::time::{Duration, sleep};

/// Returns the configured path for the unix domain socket.
fn get_socket_path() -> PathBuf {
	let socket_dir_str = getenv::get_env("SOCKET_DIR", "/var/run/vane".to_string());
	Path::new(&socket_dir_str).join("console.sock")
}

/// Binds a Unix domain socket for the management console.
///
/// Handles directory creation and cleanup of stale socket files.
pub async fn bind_unix_socket() -> Result<UnixListener, std::io::Error> {
	let socket_path = get_socket_path();

	// Ensure the socket directory exists.
	if let Some(parent_dir) = socket_path.parent() {
		if !parent_dir.exists() {
			if let Err(e) = fs::create_dir_all(parent_dir) {
				log(
					LogLevel::Error,
					&format!(
						"✗ Failed to create socket directory {}: {}",
						parent_dir.display(),
						e
					),
				);
				return Err(e); // Propagate the error.
			}
			log(
				LogLevel::Debug,
				&format!("✓ Created socket directory at: {}", parent_dir.display()),
			);
		}
	}

	// If the socket file already exists, warn and replace it.
	if socket_path.exists() {
		log(
			LogLevel::Warn,
			&format!("✗ Socket file {} already exists.", socket_path.display()),
		);
		log(
			LogLevel::Warn,
			"➜ Replacing in 5 seconds. Press Ctrl+C to abort.",
		);
		sleep(Duration::from_secs(5)).await;
		fs::remove_file(&socket_path)?; // Propagate error on failure.
	}

	// Bind the new socket.
	let listener = UnixListener::bind(&socket_path)?;
	log(
		LogLevel::Info,
		&format!(
			"✓ Management console listening on unix:{}",
			socket_path.display()
		),
	);

	Ok(listener)
}

/// Removes the Unix socket file on shutdown.
pub fn cleanup_unix_socket() {
	let socket_path = get_socket_path();
	if socket_path.exists() {
		match fs::remove_file(&socket_path) {
			Ok(_) => log(
				LogLevel::Debug,
				&format!("⚙ Cleaned up socket file: {}", socket_path.display()),
			),
			Err(e) => log(
				LogLevel::Error,
				&format!(
					"✗ Failed to clean up socket file {}: {}",
					socket_path.display(),
					e
				),
			),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;
	use temp_env;
	use tempfile::tempdir;

	/// Tests that the socket path is correctly derived from the environment or the default.
	#[test]
	#[serial]
	fn test_socket_path_resolution() {
		// Test with environment variable set
		let temp_dir = tempdir().unwrap();
		let temp_path = temp_dir.path();
		temp_env::with_var("SOCKET_DIR", Some(temp_path.to_str().unwrap()), || {
			assert_eq!(get_socket_path(), temp_path.join("console.sock"));
		});

		// Test fallback to default when environment variable is not set
		temp_env::with_var_unset("SOCKET_DIR", || {
			assert_eq!(get_socket_path(), Path::new("/var/run/vane/console.sock"));
		});
	}
}
