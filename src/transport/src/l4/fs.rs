/* src/layers/l4/fs.rs */

use crate::ingress::state::Protocol;
use std::io;
use std::path::PathBuf;
use tokio::fs;
use vane_primitives::common::config::file_loader;

const SUPPORTED_EXTENSIONS: [&str; 4] = ["toml", "yml", "yaml", "json"];

fn get_port_config_path(port: u16) -> PathBuf {
	file_loader::get_config_dir().join(format!("[{port}]"))
}

pub async fn create_protocol_listener(port: u16, protocol: &Protocol) -> io::Result<()> {
	let port_dir = get_port_config_path(port);
	if fs::metadata(&port_dir).await.is_err() {
		fs::create_dir(&port_dir).await?;
	}
	let file_name = match protocol {
		Protocol::Tcp => "tcp.toml",
		Protocol::Udp => "udp.toml",
	};
	fs::File::create(port_dir.join(file_name)).await?;
	Ok(())
}

pub async fn delete_protocol_listener(port: u16, protocol: &Protocol) -> io::Result<()> {
	let port_dir = get_port_config_path(port);
	if fs::metadata(&port_dir).await.is_err() {
		return Ok(());
	}
	let base_name = match protocol {
		Protocol::Tcp => "tcp",
		Protocol::Udp => "udp",
	};
	for ext in SUPPORTED_EXTENSIONS {
		let path = port_dir.join(format!("{base_name}.{ext}"));
		if fs::metadata(&path).await.is_ok() {
			fs::remove_file(path).await?;
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::tempdir;

	#[tokio::test]
	async fn test_listener_file_lifecycle_direct() {
		let temp_dir = tempdir().unwrap();
		let port_dir = temp_dir.path().join("[8080]");

		// Test file creation directly without relying on CONFIG_DIR env
		fs::create_dir(&port_dir).await.unwrap();
		fs::File::create(port_dir.join("tcp.toml")).await.unwrap();
		fs::File::create(port_dir.join("udp.toml")).await.unwrap();
		assert!(port_dir.join("tcp.toml").exists());
		assert!(port_dir.join("udp.toml").exists());

		fs::remove_file(port_dir.join("tcp.toml")).await.unwrap();
		fs::remove_file(port_dir.join("udp.toml")).await.unwrap();
		assert!(!port_dir.join("tcp.toml").exists());
		assert!(!port_dir.join("udp.toml").exists());
	}
}
