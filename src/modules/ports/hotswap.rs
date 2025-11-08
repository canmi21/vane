/* src/modules/ports/hotswap.rs */

use super::{
	super::server::l4::{
		loader,
		model::{TcpConfig, UdpConfig},
	},
	listener,
	model::{CONFIG_STATE, PortStatus, Protocol},
};
use crate::common::{getconf, getenv};
use fancy_log::{LogLevel, log};
use std::{collections::HashMap, fs, sync::Arc};
use tokio::sync::mpsc;

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
						let port_path = entry.path();
						let tcp_config = loader::load_config::<TcpConfig>(port, "tcp", &port_path.join("tcp"));
						let udp_config = loader::load_config::<UdpConfig>(port, "udp", &port_path.join("udp"));

						statuses.push(PortStatus {
							port,
							active: tcp_config.is_some() || udp_config.is_some(),
							tcp_config: tcp_config.map(Arc::new),
							udp_config: udp_config.map(Arc::new),
						});
					}
				}
			}
		}
	}
	statuses
}

/// Listens for update signals, calculates the config diff, and starts/stops listeners.
pub async fn listen_for_updates(mut rx: mpsc::Receiver<()>) {
	let ip_version_str =
		if getenv::get_env("LISTEN_IPV6", "false".to_string()).to_lowercase() == "true" {
			"IPv4 + IPv6"
		} else {
			"IPv4"
		};

	while rx.recv().await.is_some() {
		log(LogLevel::Info, "✓ Config change detected, diff...");
		let old_statuses = CONFIG_STATE.load();
		let new_statuses = scan_ports_config();

		type PortConfigMap = HashMap<u16, (bool, bool)>;
		let old_map: PortConfigMap = old_statuses
			.iter()
			.map(|s| (s.port, (s.tcp_config.is_some(), s.udp_config.is_some())))
			.collect();
		let new_map: PortConfigMap = new_statuses
			.iter()
			.map(|s| (s.port, (s.tcp_config.is_some(), s.udp_config.is_some())))
			.collect();

		let mut has_changes = false;

		for (port, (new_tcp, new_udp)) in &new_map {
			if let Some((old_tcp, old_udp)) = old_map.get(port) {
				if *new_tcp && !*old_tcp {
					log(
						LogLevel::Info,
						&format!("↑ {} PORT {} TCP UP", ip_version_str, port),
					);
					listener::start_listener(*port, Protocol::Tcp);
					has_changes = true;
				}
				if !*new_tcp && *old_tcp {
					log(
						LogLevel::Info,
						&format!("↓ {} PORT {} TCP DOWN", ip_version_str, port),
					);
					listener::stop_listener(*port, Protocol::Tcp);
					has_changes = true;
				}
				if *new_udp && !*old_udp {
					log(
						LogLevel::Info,
						&format!("↑ {} PORT {} UDP UP", ip_version_str, port),
					);
					listener::start_listener(*port, Protocol::Udp);
					has_changes = true;
				}
				if !*new_udp && *old_udp {
					log(
						LogLevel::Info,
						&format!("↓ {} PORT {} UDP DOWN", ip_version_str, port),
					);
					listener::stop_listener(*port, Protocol::Udp);
					has_changes = true;
				}
			} else {
				if *new_tcp {
					log(
						LogLevel::Info,
						&format!("↑ {} PORT {} TCP UP", ip_version_str, port),
					);
					listener::start_listener(*port, Protocol::Tcp);
					has_changes = true;
				}
				if *new_udp {
					log(
						LogLevel::Info,
						&format!("↑ {} PORT {} UDP UP", ip_version_str, port),
					);
					listener::start_listener(*port, Protocol::Udp);
					has_changes = true;
				}
			}
		}

		for (port, (old_tcp, old_udp)) in &old_map {
			if !new_map.contains_key(port) {
				if *old_tcp {
					log(
						LogLevel::Info,
						&format!("↓ {} PORT {} TCP DOWN", ip_version_str, port),
					);
					listener::stop_listener(*port, Protocol::Tcp);
					has_changes = true;
				}
				if *old_udp {
					log(
						LogLevel::Info,
						&format!("↓ {} PORT {} UDP DOWN", ip_version_str, port),
					);
					listener::stop_listener(*port, Protocol::Udp);
					has_changes = true;
				}
			}
		}

		if !has_changes {
			log(LogLevel::Debug, "⚙ No effective changes detected.");
		}
		CONFIG_STATE.store(Arc::new(new_statuses));
	}
}
