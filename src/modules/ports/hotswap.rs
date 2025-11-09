/* src/modules/ports/hotswap.rs */

use super::{
	super::stack::transport::{
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

/// Scans the 'listener' config subdirectory for port configurations.
///
/// This function reads each subdirectory within `CONFIG_DIR/listener/` that is
/// named like `[<port>]`, and attempts to load `tcp.{toml|yaml|json}` and
/// `udp.{toml|yaml|json}` files within it. It returns a vector of `PortStatus`
/// representing the discovered configurations.
pub fn scan_ports_config() -> Vec<PortStatus> {
	let listener_dir = getconf::get_config_dir().join("listener");
	let mut statuses = Vec::new();

	if !listener_dir.exists() || !listener_dir.is_dir() {
		return statuses;
	}

	if let Ok(entries) = fs::read_dir(listener_dir) {
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
///
/// This async function waits on a channel for a signal that the configuration
/// has changed. Upon receiving a signal, it re-scans the port configurations,
/// compares the new state with the old one, and issues commands to start or
/// stop TCP/UDP listeners accordingly.
pub async fn listen_for_updates(mut rx: mpsc::Receiver<()>) {
	let ip_version_str =
		if getenv::get_env("LISTEN_IPV6", "false".to_string()).to_lowercase() == "true" {
			"IPv4 + IPv6"
		} else {
			"IPv4"
		};

	while rx.recv().await.is_some() {
		log(
			LogLevel::Info,
			"➜ Config change signal received, diffing listeners...",
		);
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
			log(LogLevel::Debug, "⚙ No effective listener changes detected.");
		}
		CONFIG_STATE.store(Arc::new(new_statuses));
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs;
	use temp_env;
	use tempfile::tempdir;

	// A minimal, but structurally valid, TCP config for testing purposes.
	const DUMMY_TCP_CONFIG: &str = r#"
[[protocols]]
name = "catchall"
priority = 1
detect = { method = "fallback", pattern = "any" }
destination = { type = "forward", forward = { strategy = "random", targets = [{ ip = "127.0.0.1", port = 1 }] } }
"#;

	// A minimal, but structurally valid, UDP config for testing purposes.
	// CORRECTED: Used valid TOML string escape sequences for the pattern.
	const DUMMY_UDP_CONFIG: &str = r#"
[[protocols]]
name = "dns"
priority = 1
detect = { method = "prefix", pattern = "\u0000\u0001" }
destination = { type = "forward", forward = { strategy = "random", targets = [{ ip = "1.1.1.1", port = 53 }] } }
"#;

	/// Tests the port scanning logic under various filesystem conditions.
	#[test]
	#[serial_test::serial]
	fn test_scan_ports_logic() {
		let temp_dir = tempdir().unwrap();
		let config_path = temp_dir.path();
		let listener_path = config_path.join("listener");
		fs::create_dir(&listener_path).unwrap();

		temp_env::with_var("CONFIG_DIR", Some(config_path.to_str().unwrap()), || {
			// 1. No port directories exist, should return empty.
			let statuses = scan_ports_config();
			assert!(statuses.is_empty());

			// 2. Create a full setup:
			// - Port 8080 with TCP only
			fs::create_dir(listener_path.join("[8080]")).unwrap();
			fs::write(listener_path.join("[8080]/tcp.toml"), DUMMY_TCP_CONFIG).unwrap();
			// - Port 9090 with UDP only
			fs::create_dir(listener_path.join("[9090]")).unwrap();
			fs::write(listener_path.join("[9090]/udp.toml"), DUMMY_UDP_CONFIG).unwrap();
			// - Port 9999 with an empty directory (inactive)
			fs::create_dir(listener_path.join("[9999]")).unwrap();
			// - A non-port file to be ignored
			fs::write(listener_path.join("readme.txt"), "ignore me").unwrap();

			// 3. Scan again and verify the results.
			let mut statuses = scan_ports_config();
			// Sort by port to make assertions predictable.
			statuses.sort_by_key(|s| s.port);

			assert_eq!(statuses.len(), 3);

			// Verify Port 8080
			let s8080 = statuses.get(0).unwrap();
			assert_eq!(s8080.port, 8080);
			assert!(
				s8080.active,
				"Port 8080 should be active with a valid TCP config"
			);
			assert!(s8080.tcp_config.is_some());
			assert!(s8080.udp_config.is_none());

			// Verify Port 9090
			let s9090 = statuses.get(1).unwrap();
			assert_eq!(s9090.port, 9090);
			assert!(
				s9090.active,
				"Port 9090 should be active with a valid UDP config"
			);
			assert!(s9090.tcp_config.is_none());
			assert!(s9090.udp_config.is_some());

			// Verify Port 9999
			let s9999 = statuses.get(2).unwrap();
			assert_eq!(s9999.port, 9999);
			assert!(
				!s9999.active,
				"Port 9999 should be inactive with no config files"
			);
			assert!(s9090.tcp_config.is_none());
			assert!(s9090.udp_config.is_some());
		});
	}
}
