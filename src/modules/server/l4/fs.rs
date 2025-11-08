/* src/modules/server/l4/fs.rs */

use crate::common::getconf;
use crate::modules::ports::model::Protocol;
use std::{fs, io, path::PathBuf};

// The list of supported config file extensions.
const SUPPORTED_EXTENSIONS: [&str; 3] = ["toml", "yaml", "json"];

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
