/* src/modules/ports/hotswap.rs */

use super::{
	listener,
	model::{PortState, PortStatus, Protocol},
};
use crate::common::getconf;
use crate::common::getenv;
use fancy_log::{LogLevel, log};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Returns the filesystem path for a given port's configuration directory.
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
	fs::File::create(port_dir.join(protocol_str))?;
	Ok(())
}

/// Deletes a protocol-specific listener config file.
pub fn delete_protocol_listener(port: u16, protocol: &Protocol) -> std::io::Result<()> {
	let port_dir = get_port_config_path(port);
	if !port_dir.exists() {
		return Ok(());
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

/// Scans the configuration directory and builds a complete list of port statuses.
pub fn scan_ports_config() -> Vec<PortStatus> {
	let config_dir = getconf::get_config_dir();
	let mut statuses = Vec::new();
	if let Ok(entries) = fs::read_dir(config_dir) {
		for entry in entries.flatten() {
			if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
				continue;
			}
			if let Some(name) = entry.file_name().to_str() {
				if name.starts_with('[') && name.ends_with(']') {
					if let Ok(port) = name[1..name.len() - 1].parse::<u16>() {
						let mut protocols = Vec::new();
						let port_config_path = entry.path();
						if port_config_path.join("tcp.yaml").exists() {
							protocols.push(Protocol::Tcp);
						}
						if port_config_path.join("udp.yaml").exists() {
							protocols.push(Protocol::Udp);
						}
						statuses.push(PortStatus {
							port,
							active: !protocols.is_empty(),
							protocols,
						});
					}
				}
			}
		}
	}
	statuses
}

/// Listens for update signals, calculates the config diff, and starts/stops listeners.
pub async fn listen_for_updates(state: PortState, mut rx: mpsc::Receiver<()>) {
	// Determine the IP version string once when the task starts.
	let ip_version_str =
		if getenv::get_env("LISTEN_IPV6", "false".to_string()).to_lowercase() == "true" {
			"IPv4 + IPv6"
		} else {
			"IPv4"
		};

	while rx.recv().await.is_some() {
		log(LogLevel::Info, "✓ Config change detected, diff...");
		let old_statuses = state.load();
		let new_statuses = scan_ports_config();
		let old_map: HashMap<u16, HashSet<Protocol>> = old_statuses
			.iter()
			.map(|s| (s.port, s.protocols.iter().cloned().collect()))
			.collect();
		let new_map: HashMap<u16, HashSet<Protocol>> = new_statuses
			.iter()
			.map(|s| (s.port, s.protocols.iter().cloned().collect()))
			.collect();
		let mut has_changes = false;

		for (port, new_protocols) in &new_map {
			let format_protocol = |p: &Protocol| format!("{:?}", p).to_uppercase();
			match old_map.get(port) {
				Some(old_protocols) => {
					for p in new_protocols.difference(old_protocols) {
						log(
							LogLevel::Info,
							&format!(
								"↑ {} PORT {} {} UP",
								ip_version_str,
								port,
								format_protocol(p)
							),
						);
						has_changes = true;
						listener::start_listener(*port, p.clone());
					}
				}
				None => {
					for p in new_protocols {
						log(
							LogLevel::Info,
							&format!(
								"↑ {} PORT {} {} UP",
								ip_version_str,
								port,
								format_protocol(p)
							),
						);
						has_changes = true;
						listener::start_listener(*port, p.clone());
					}
				}
			}
		}

		for (port, old_protocols) in &old_map {
			let format_protocol = |p: &Protocol| format!("{:?}", p).to_uppercase();
			match new_map.get(port) {
				Some(new_protocols) => {
					for p in old_protocols.difference(new_protocols) {
						log(
							LogLevel::Info,
							&format!(
								"↓ {} PORT {} {} DOWN",
								ip_version_str,
								port,
								format_protocol(p)
							),
						);
						has_changes = true;
						listener::stop_listener(*port, p.clone());
					}
				}
				None => {
					for p in old_protocols {
						log(
							LogLevel::Info,
							&format!(
								"↓ {} PORT {} {} DOWN",
								ip_version_str,
								port,
								format_protocol(p)
							),
						);
						has_changes = true;
						listener::stop_listener(*port, p.clone());
					}
				}
			}
		}

		if !has_changes {
			log(LogLevel::Debug, "⚙ No effective changes detected.");
		}
		state.store(Arc::new(new_statuses));
	}
}
