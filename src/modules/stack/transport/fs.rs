/* src/modules/stack/transport/fs.rs */

use crate::common::getconf;
use crate::modules::ports::model::Protocol;
use std::{fs, io, path::PathBuf};

// The list of supported config file extensions.
const SUPPORTED_EXTENSIONS: [&str; 4] = ["toml", "yml", "yaml", "json"];

/// Returns the filesystem path for a given port's configuration directory.
fn get_port_config_path(port: u16) -> PathBuf {
	getconf::get_config_dir().join(format!("[{}]", port))
}

/// Creates a default, empty listener config file.
pub fn create_protocol_listener(port: u16, protocol: &Protocol) -> io::Result<()> {
	let port_dir = get_port_config_path(port);
	if !port_dir.exists() {
		fs::create_dir(&port_dir)?;
	}
	let file_name = match protocol {
		Protocol::Tcp => "tcp.toml",
		Protocol::Udp => "udp.toml",
	};
	fs::File::create(port_dir.join(file_name))?;
	Ok(())
}

/// Deletes all possible config files for a protocol.
pub fn delete_protocol_listener(port: u16, protocol: &Protocol) -> io::Result<()> {
	let port_dir = get_port_config_path(port);
	if !port_dir.exists() {
		return Ok(());
	}
	let base_name = match protocol {
		Protocol::Tcp => "tcp",
		Protocol::Udp => "udp",
	};
	for ext in SUPPORTED_EXTENSIONS {
		let path = port_dir.join(format!("{}.{}", base_name, ext));
		if path.exists() {
			fs::remove_file(path)?;
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;
	use temp_env;
	use tempfile::tempdir;

	/// Tests the full lifecycle: creating and then deleting listener configuration files.
	#[test]
	#[serial]
	fn test_listener_file_lifecycle() {
		let temp_dir = tempdir().unwrap();
		let config_path = temp_dir.path();
		let port = 8080;
		let port_dir = config_path.join(format!("[{}]", port));

		temp_env::with_var("CONFIG_DIR", Some(config_path.to_str().unwrap()), || {
			// 1. Initially, the port directory should not exist.
			assert!(!port_dir.exists());

			// 2. Create a TCP listener file.
			assert!(create_protocol_listener(port, &Protocol::Tcp).is_ok());
			assert!(port_dir.exists(), "Port directory should be created.");
			assert!(
				port_dir.join("tcp.toml").exists(),
				"Default .toml file should be created for TCP."
			);

			// 3. Create other variants of the TCP config to test comprehensive deletion.
			fs::File::create(port_dir.join("tcp.yaml")).unwrap();
			assert!(port_dir.join("tcp.yaml").exists());

			// 4. Create a UDP listener file in the same port directory.
			assert!(create_protocol_listener(port, &Protocol::Udp).is_ok());
			assert!(port_dir.join("udp.toml").exists());

			// 5. Delete only the TCP listener files.
			assert!(delete_protocol_listener(port, &Protocol::Tcp).is_ok());
			assert!(
				!port_dir.join("tcp.toml").exists(),
				".toml file should be deleted."
			);
			assert!(
				!port_dir.join("tcp.yaml").exists(),
				".yaml file should also be deleted."
			);
			assert!(
				port_dir.join("udp.toml").exists(),
				"UDP file should not be affected."
			);

			// 6. Delete the UDP listener file.
			assert!(delete_protocol_listener(port, &Protocol::Udp).is_ok());
			assert!(!port_dir.join("udp.toml").exists());
		});
	}

	/// Tests that deletion functions do not fail when files or directories are already absent.
	#[test]
	#[serial]
	fn test_delete_is_idempotent() {
		let temp_dir = tempdir().unwrap();
		let config_path = temp_dir.path();
		let port = 9090;

		temp_env::with_var("CONFIG_DIR", Some(config_path.to_str().unwrap()), || {
			// Attempt to delete a listener for a port that has no directory.
			// This should complete successfully without returning an error.
			let result = delete_protocol_listener(port, &Protocol::Tcp);
			assert!(result.is_ok());
		});
	}
}
