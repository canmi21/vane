/* src/layers/l4/fs.rs */

use crate::common::config::file_loader;
use crate::ingress::state::Protocol;
use std::io;
use std::path::PathBuf;
use tokio::fs;

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
	use serial_test::serial;
	use tempfile::tempdir;

	#[tokio::test]
	#[serial]
	async fn test_listener_file_lifecycle() {
		let temp_dir = tempdir().unwrap();
		let config_path = temp_dir.path();
		let port = 8080;
		let _port_dir = config_path.join(format!("[{}]", port));

		temp_env::with_var(
			"CONFIG_DIR",
			Some(config_path.to_str().unwrap()),
			|| async move {
				assert!(create_protocol_listener(port, &Protocol::Tcp).await.is_ok());
				assert!(create_protocol_listener(port, &Protocol::Udp).await.is_ok());
				assert!(delete_protocol_listener(port, &Protocol::Tcp).await.is_ok());
				assert!(delete_protocol_listener(port, &Protocol::Udp).await.is_ok());
			},
		)
		.await;
	}
}
