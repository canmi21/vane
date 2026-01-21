/* src/ingress/hotswap.rs */

use super::{
	listener,
	state::{CONFIG_STATE, PortStatus, Protocol},
};
use crate::common::config::loader::LoadResult;
use crate::common::{
	config::{env_loader, file_loader},
	sys::hotswap::watch_loop,
};
use crate::layers::l4::{loader, tcp::TcpConfig, udp::UdpConfig};
use fancy_log::{LogLevel, log};
use std::{collections::HashMap, sync::Arc};
use tokio::fs;
use tokio::sync::mpsc;

/// Scans the 'listener' config subdirectory for port configurations.
pub async fn scan_ports_config(current_state: &[PortStatus]) -> Vec<PortStatus> {
	let listener_dir = file_loader::get_config_dir().join("listener");
	let mut statuses = Vec::new();
	let current_map: HashMap<u16, &PortStatus> = current_state.iter().map(|s| (s.port, s)).collect();

	if let Ok(metadata) = fs::metadata(&listener_dir).await {
		if !metadata.is_dir() {
			return statuses;
		}
	} else {
		return statuses;
	}

	if let Ok(mut entries) = fs::read_dir(listener_dir).await {
		while let Ok(Some(entry)) = entries.next_entry().await {
			if let Ok(m) = entry.metadata().await {
				if !m.is_dir() {
					continue;
				}
			} else {
				continue;
			}

			if let Some(name) = entry.file_name().to_str()
				&& name.starts_with('[')
				&& name.ends_with(']')
				&& let Ok(port) = name[1..name.len() - 1].parse::<u16>()
			{
				let port_path = entry.path();
				let new_tcp = loader::load_config::<TcpConfig>("tcp", &port_path.join("tcp")).await;
				let new_udp = loader::load_config::<UdpConfig>("udp", &port_path.join("udp")).await;
				let old_status = current_map.get(&port);

				let tcp_config = match new_tcp {
					LoadResult::Ok(cfg) => Some(Arc::new(cfg)),
					LoadResult::NotFound => None,
					LoadResult::Invalid => {
						if let Some(old) = old_status {
							if old.tcp_config.is_some() {
								log(
									LogLevel::Warn,
									&format!(
										"⚠ New TCP config for port {port} is invalid. Keeping last known good version."
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
										"⚠ New UDP config for port {port} is invalid. Keeping last known good version."
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
	statuses
}

/// Listens for update signals, calculates the config diff, and starts/stops listeners.
pub async fn listen_for_updates(rx: mpsc::Receiver<()>) {
	let ip_version_str =
		if env_loader::get_env("LISTEN_IPV6", "false".to_owned()).to_lowercase() == "true" {
			"IPv4 + IPv6"
		} else {
			"IPv4"
		};

	watch_loop(rx, "Listeners", || async {
		log(LogLevel::Debug, "⚙ Diffing listeners...");
		let old_statuses = CONFIG_STATE.load();
		let new_statuses = scan_ports_config(&old_statuses).await;

		CONFIG_STATE.store(Arc::new(new_statuses.clone()));

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
				// TCP
				if new_tcp != old_tcp {
					if let (Some(nt), Some(ot)) = (new_tcp, old_tcp) {
						if nt != ot {
							log(
								LogLevel::Info,
								&format!("↻ {ip_version_str} PORT {port} TCP RELOAD (Config Changed)"),
							);
							listener::stop_listener(*port, Protocol::Tcp);
							listener::start_listener(*port, Protocol::Tcp);
						}
					} else if new_tcp.is_some() {
						log(
							LogLevel::Info,
							&format!("↑ {ip_version_str} PORT {port} TCP UP"),
						);
						listener::start_listener(*port, Protocol::Tcp);
					} else {
						log(
							LogLevel::Info,
							&format!("↓ {ip_version_str} PORT {port} TCP DOWN"),
						);
						listener::stop_listener(*port, Protocol::Tcp);
					}
					has_changes = true;
				}
				// UDP
				if new_udp != old_udp {
					if let (Some(nu), Some(ou)) = (new_udp, old_udp) {
						if nu != ou {
							log(
								LogLevel::Info,
								&format!("↻ {ip_version_str} PORT {port} UDP RELOAD (Config Changed)"),
							);
							listener::stop_listener(*port, Protocol::Udp);
							listener::start_listener(*port, Protocol::Udp);
						}
					} else if new_udp.is_some() {
						log(
							LogLevel::Info,
							&format!("↑ {ip_version_str} PORT {port} UDP UP"),
						);
						listener::start_listener(*port, Protocol::Udp);
					} else {
						log(
							LogLevel::Info,
							&format!("↓ {ip_version_str} PORT {port} UDP DOWN"),
						);
						listener::stop_listener(*port, Protocol::Udp);
					}
					has_changes = true;
				}
			} else {
				// Purely new
				if new_tcp.is_some() {
					log(
						LogLevel::Info,
						&format!("↑ {ip_version_str} PORT {port} TCP UP"),
					);
					listener::start_listener(*port, Protocol::Tcp);
					has_changes = true;
				}
				if new_udp.is_some() {
					log(
						LogLevel::Info,
						&format!("↑ {ip_version_str} PORT {port} UDP UP"),
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
						&format!("↓ {ip_version_str} PORT {port} TCP DOWN"),
					);
					listener::stop_listener(*port, Protocol::Tcp);
					has_changes = true;
				}
				if old_udp.is_some() {
					log(
						LogLevel::Info,
						&format!("↓ {ip_version_str} PORT {port} UDP DOWN"),
					);
					listener::stop_listener(*port, Protocol::Udp);
					has_changes = true;
				}
			}
		}

		if !has_changes {
			log(LogLevel::Debug, "⚙ No effective listener changes.");
		}
	})
	.await;
}
