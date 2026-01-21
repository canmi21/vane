/* src/bootstrap/socket.rs */

use crate::common::config::env_loader;
use fancy_log::{LogLevel, log};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::net::UnixListener;
use tokio::time::{Duration, sleep};

fn get_socket_path() -> PathBuf {
	let socket_dir_str = env_loader::get_env("SOCKET_DIR", "/var/run/vane".to_owned());
	Path::new(&socket_dir_str).join("console.sock")
}

pub async fn bind_unix_socket() -> Result<UnixListener, std::io::Error> {
	let socket_path = get_socket_path();
	if let Some(parent_dir) = socket_path.parent()
		&& fs::metadata(parent_dir).await.is_err()
	{
		fs::create_dir_all(parent_dir).await?;
	}
	if fs::metadata(&socket_path).await.is_ok() {
		log(
			LogLevel::Warn,
			&format!(
				"✗ Socket {} exists. Replacing in 5s.",
				socket_path.display()
			),
		);
		sleep(Duration::from_secs(5)).await;
		let _ = fs::remove_file(&socket_path).await;
	}
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

pub async fn cleanup_unix_socket() {
	let socket_path = get_socket_path();
	if fs::metadata(&socket_path).await.is_ok() {
		let _ = fs::remove_file(&socket_path).await;
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;
	use tempfile::tempdir;

	#[test]
	#[serial]
	fn test_socket_path_resolution() {
		let temp_dir = tempdir().unwrap();
		let temp_path = temp_dir.path();
		let temp_path_str = temp_path.to_str().unwrap();

		temp_env::with_var("SOCKET_DIR", Some(temp_path_str), || {
			assert_eq!(get_socket_path(), temp_path.join("console.sock"));
		});
	}
}
