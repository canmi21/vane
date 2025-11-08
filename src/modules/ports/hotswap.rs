/* src/modules/ports/hotswap.rs */

use super::model::{PortState, PortStatus, Protocol};
use crate::common::getconf;
use fancy_log::{LogLevel, log};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Scans the configuration directory and builds a complete list of port statuses.
pub fn scan_ports_config() -> Vec<PortStatus> {
	let config_dir = getconf::get_config_dir();
	let mut statuses = Vec::new();

	if let Ok(entries) = std::fs::read_dir(config_dir) {
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

/// A task that listens for update signals and triggers a configuration reload.
pub async fn listen_for_updates(state: PortState, mut rx: mpsc::Receiver<()>) {
	// This loop waits for a signal from the file watcher.
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

		// --- Check for new and modified ports/protocols ---
		for (port, new_protocols) in &new_map {
			// Format the protocol string for logging.
			let format_protocol = |p: &Protocol| format!("{:?}", p).to_uppercase();

			match old_map.get(port) {
				// Port existed before, check for protocol changes.
				Some(old_protocols) => {
					// Check for added protocols (UP)
					for p in new_protocols.difference(old_protocols) {
						log(
							LogLevel::Info,
							&format!("↑ PORT {} {} UP", port, format_protocol(p)),
						);
						has_changes = true;
					}
					// Check for removed protocols (DOWN)
					for p in old_protocols.difference(new_protocols) {
						log(
							LogLevel::Info,
							&format!("↓ PORT {} {} DOWN", port, format_protocol(p)),
						);
						has_changes = true;
					}
				}
				// This is a completely new port configuration.
				None => {
					for p in new_protocols {
						log(
							LogLevel::Info,
							&format!("↑ PORT {} {} UP", port, format_protocol(p)),
						);
					}
					has_changes = !new_protocols.is_empty();
				}
			}
		}

		// --- Check for completely removed ports ---
		for (port, old_protocols) in &old_map {
			if !new_map.contains_key(port) {
				let format_protocol = |p: &Protocol| format!("{:?}", p).to_uppercase();
				for p in old_protocols {
					log(
						LogLevel::Info,
						&format!("↓ PORT {} {} DOWN", port, format_protocol(p)),
					);
				}
				has_changes = !old_protocols.is_empty();
			}
		}

		if !has_changes {
			log(LogLevel::Debug, "⚙ No effective changes detected.");
		} else {
			// TODO: Based on the diff, actually start/stop the real network listeners here.
		}

		// Atomically swap the old state with the new one.
		state.store(Arc::new(new_statuses));
	}
}
