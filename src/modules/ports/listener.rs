/* src/modules/ports/listener.rs */

use std::fs;
use std::path::PathBuf;

// Add this import to bring the Protocol enum into scope.
use super::model::Protocol;
use crate::common::getconf;

// Helper to get the path for a specific port's directory.
fn get_port_config_path(port: u16) -> PathBuf {
	getconf::get_config_dir().join(format!("[{}]", port))
}

/// Creates a protocol-specific listener config file (e.g., tcp.yaml).
pub fn create_protocol_listener(port: u16, protocol: &Protocol) -> std::io::Result<()> {
	let port_dir = get_port_config_path(port);
	if !port_dir.exists() {
		fs::create_dir(&port_dir)?;
	}

	let protocol_str = match protocol {
		Protocol::Tcp => "tcp.yaml",
		Protocol::Udp => "udp.yaml",
	};

	let config_file_path = port_dir.join(protocol_str);
	fs::File::create(config_file_path)?;

	Ok(())
}

/// Deletes a protocol-specific listener config file.
pub fn delete_protocol_listener(port: u16, protocol: &Protocol) -> std::io::Result<()> {
	let port_dir = get_port_config_path(port);
	if !port_dir.exists() {
		return Ok(()); // Nothing to delete.
	}

	let protocol_str = match protocol {
		Protocol::Tcp => "tcp.yaml",
		Protocol::Udp => "udp.yaml",
	};

	let config_file_path = port_dir.join(protocol_str);
	if config_file_path.exists() {
		fs::remove_file(config_file_path)?;
	}

	Ok(())
}
