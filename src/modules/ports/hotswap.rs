/* src/modules/ports/hotswap.rs */

use super::{
	super::stack::transport::{loader, tcp::TcpConfig, udp::UdpConfig},
	listener,
	model::{CONFIG_STATE, PortStatus, Protocol},
};
use crate::common::loader::LoadResult;
use crate::common::{getconf, getenv, hotswap::watch_loop};
use fancy_log::{LogLevel, log};
use std::{collections::HashMap, fs, sync::Arc};
use tokio::sync::mpsc;

/// Scans the 'listener' config subdirectory for port configurations.
/// Implements Keep-Last-Known-Good (KLKG) strategy by referencing `current_state`.
pub fn scan_ports_config(current_state: &[PortStatus]) -> Vec<PortStatus> {
	let listener_dir = getconf::get_config_dir().join("listener");
	let mut statuses = Vec::new();

	// Create a lookup map for the current state to facilitate KLKG
	let current_map: HashMap<u16, &PortStatus> = current_state.iter().map(|s| (s.port, s)).collect();

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

						// Try loading new configs
						let new_tcp = loader::load_config::<TcpConfig>("tcp", &port_path.join("tcp"));
						let new_udp = loader::load_config::<UdpConfig>("udp", &port_path.join("udp"));

						// KLKG vs Unload Logic:
						let old_status = current_map.get(&port);

						let tcp_config = match new_tcp {
							LoadResult::Ok(cfg) => Some(Arc::new(cfg)),
							LoadResult::NotFound => None, // File is gone, user wants to unload
							LoadResult::Invalid => {
								// File exists but is broken, try to recover from old state
								if let Some(old) = old_status {
									if old.tcp_config.is_some() {
										log(
											LogLevel::Warn,
											&format!(
												"⚠ New TCP config for port {} is invalid. Keeping last known good version.",
												port
											),
										);
									}
									old.tcp_config.clone()
								} else {
									None
								}
							}
						};

						let udp_config = match new_udp {
							LoadResult::Ok(cfg) => Some(Arc::new(cfg)),
							LoadResult::NotFound => None,
							LoadResult::Invalid => {
								if let Some(old) = old_status {
									if old.udp_config.is_some() {
										log(
											LogLevel::Warn,
											&format!(
												"⚠ New UDP config for port {} is invalid. Keeping last known good version.",
												port
											),
										);
									}
									old.udp_config.clone()
								} else {
									None
								}
							}
						};

						statuses.push(PortStatus {
							port,
							active: tcp_config.is_some() || udp_config.is_some(),
							tcp_config,
							udp_config,
						});
					}
				}
			}
		}
	}
	statuses
}

/// Listens for update signals, calculates the config diff, and starts/stops listeners.
pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	let ip_version_str =
		if getenv::get_env("LISTEN_IPV6", "false".to_string()).to_lowercase() == "true" {
			"IPv4 + IPv6"
		} else {
			"IPv4"
		};

	watch_loop(rx, "Listeners", || async {
		log(LogLevel::Debug, "⚙ Diffing listeners...");
		let old_statuses = CONFIG_STATE.load();
		let new_statuses = scan_ports_config(&old_statuses);

		// We update the global state *before* restarting listeners so that new tasks
		// pick up the new config immediately.
		CONFIG_STATE.store(Arc::new(new_statuses.clone()));

		// Use a Map that stores the full config for comparison.
		// Key: Port
		// Value: (Option<Arc<TcpConfig>>, Option<Arc<UdpConfig>>)
		type ConfigMap = HashMap<u16, (Option<Arc<TcpConfig>>, Option<Arc<UdpConfig>>)>;

		let old_map: ConfigMap = old_statuses
			.iter()
			.map(|s| (s.port, (s.tcp_config.clone(), s.udp_config.clone())))
			.collect();

		let new_map: ConfigMap = new_statuses
			.iter()
			.map(|s| (s.port, (s.tcp_config.clone(), s.udp_config.clone())))
			.collect();

		let mut has_changes = false;

		// 1. Check for New or Modified ports
		for (port, (new_tcp, new_udp)) in &new_map {
			if let Some((old_tcp, old_udp)) = old_map.get(port) {
				// --- TCP Logic ---
				if new_tcp.is_some() && old_tcp.is_none() {
					// Added
					log(
						LogLevel::Info,
						&format!("↑ {} PORT {} TCP UP", ip_version_str, port),
					);
					listener::start_listener(*port, Protocol::Tcp);
					has_changes = true;
				} else if new_tcp.is_none() && old_tcp.is_some() {
					// Removed
					log(
						LogLevel::Info,
						&format!("↓ {} PORT {} TCP DOWN", ip_version_str, port),
					);
					listener::stop_listener(*port, Protocol::Tcp);
					has_changes = true;
				} else if let (Some(new_c), Some(old_c)) = (new_tcp, old_tcp) {
					// Both exist, check for content changes
					if new_c != old_c {
						log(
							LogLevel::Info,
							&format!(
								"↻ {} PORT {} TCP RELOAD (Config Changed)",
								ip_version_str, port
							),
						);
						// Restart to apply new config
						listener::stop_listener(*port, Protocol::Tcp);
						listener::start_listener(*port, Protocol::Tcp);
						has_changes = true;
					}
				}

				// --- UDP Logic ---
				if new_udp.is_some() && old_udp.is_none() {
					// Added
					log(
						LogLevel::Info,
						&format!("↑ {} PORT {} UDP UP", ip_version_str, port),
					);
					listener::start_listener(*port, Protocol::Udp);
					has_changes = true;
				} else if new_udp.is_none() && old_udp.is_some() {
					// Removed
					log(
						LogLevel::Info,
						&format!("↓ {} PORT {} UDP DOWN", ip_version_str, port),
					);
					listener::stop_listener(*port, Protocol::Udp);
					has_changes = true;
				} else if let (Some(new_c), Some(old_c)) = (new_udp, old_udp) {
					// Both exist, check for content changes
					if new_c != old_c {
						log(
							LogLevel::Info,
							&format!(
								"↻ {} PORT {} UDP RELOAD (Config Changed)",
								ip_version_str, port
							),
						);
						// Restart to apply new config
						listener::stop_listener(*port, Protocol::Udp);
						listener::start_listener(*port, Protocol::Udp);
						has_changes = true;
					}
				}
			} else {
				// Port didn't exist before, purely new
				if new_tcp.is_some() {
					log(
						LogLevel::Info,
						&format!("↑ {} PORT {} TCP UP", ip_version_str, port),
					);
					listener::start_listener(*port, Protocol::Tcp);
					has_changes = true;
				}
				if new_udp.is_some() {
					log(
						LogLevel::Info,
						&format!("↑ {} PORT {} UDP UP", ip_version_str, port),
					);
					listener::start_listener(*port, Protocol::Udp);
					has_changes = true;
				}
			}
		}

		// 2. Check for Completely Removed ports
		for (port, (old_tcp, old_udp)) in &old_map {
			if !new_map.contains_key(port) {
				if old_tcp.is_some() {
					log(
						LogLevel::Info,
						&format!("↓ {} PORT {} TCP DOWN", ip_version_str, port),
					);
					listener::stop_listener(*port, Protocol::Tcp);
					has_changes = true;
				}
				if old_udp.is_some() {
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
	})
	.await;
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
	const DUMMY_UDP_CONFIG: &str = r#"
[[protocols]]
name = "dns"
priority = 1
detect = { method = "prefix", pattern = "\u0000\u0001" }
destination = { type = "forward", forward = { strategy = "random", targets = [{ ip = "1.1.1.1", port = 53 }] } }
"#;

	#[test]
	#[serial_test::serial]
	fn test_scan_ports_logic() {
		let temp_dir = tempdir().unwrap();
		let config_path = temp_dir.path();
		let listener_path = config_path.join("listener");
		fs::create_dir(&listener_path).unwrap();

		temp_env::with_var("CONFIG_DIR", Some(config_path.to_str().unwrap()), || {
			// 1. No port directories exist, should return empty.
			let statuses = scan_ports_config(&[]);
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
			let mut statuses = scan_ports_config(&[]);
			statuses.sort_by_key(|s| s.port);

			assert_eq!(statuses.len(), 3);

			let s8080 = statuses.get(0).unwrap();
			assert_eq!(s8080.port, 8080);
			assert!(s8080.active);
			assert!(s8080.tcp_config.is_some());
			assert!(s8080.udp_config.is_none());

			let s9090 = statuses.get(1).unwrap();
			assert_eq!(s9090.port, 9090);
			assert!(s9090.active);
			assert!(s9090.tcp_config.is_none());
			assert!(s9090.udp_config.is_some());

			let s9999 = statuses.get(2).unwrap();
			assert_eq!(s9999.port, 9999);
			assert!(!s9999.active);
			assert!(s9999.tcp_config.is_none());
			assert!(s9999.udp_config.is_none());
		});
	}
}
