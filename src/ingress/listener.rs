/* src/ingress/listener.rs */

use super::state::{CONFIG_STATE, ListenerState, Protocol, RunningListener, TASK_REGISTRY};
use crate::common::config::env_loader;
use crate::ingress::hotswap::scan_ports_config;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Mutex;

pub fn start_listener(port: u16, protocol: Protocol) {
	let key = (port, protocol.clone());
	if TASK_REGISTRY.contains_key(&key) {
		return;
	}

	tokio::spawn(async move {
		let listen_ipv6 = env_loader::get_env("LISTEN_IPV6", "false".to_string()).to_lowercase() == "true";
		let addr: std::net::SocketAddr = if listen_ipv6 {
			([0; 8], port).into()
		} else {
			([0; 4], port).into()
		};

		log(
			LogLevel::Info,
			&format!("⚙ Binding {:?} listener on {}...", protocol, addr),
		);

		let shutdown_tx = match protocol {
			Protocol::Tcp => match TcpListener::bind(addr).await {
				Ok(l) => Some(super::tcp::spawn_tcp_listener_task(port, l)),
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ TCP bind failed on {}: {}", addr, e),
					);
					None
				}
			},
			Protocol::Udp => match UdpSocket::bind(addr).await {
				Ok(s) => Some(super::udp::spawn_udp_listener_task(port, s)),
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ UDP bind failed on {}: {}", addr, e),
					);
					None
				}
			},
		};

		if let Some(tx) = shutdown_tx {
			TASK_REGISTRY.insert(
				key,
				RunningListener {
					state: Arc::new(Mutex::new(ListenerState::Active)),
					shutdown_tx: tx,
				},
			);
			log(
				LogLevel::Info,
				&format!("✓ {:?} listener on port {} is UP", protocol, port),
			);
		}
	});
}

pub fn stop_listener(port: u16, protocol: Protocol) {
	if let Some((_, task)) = TASK_REGISTRY.remove(&(port, protocol)) {
		let _ = task.shutdown_tx.send(());
	}
}

pub async fn is_port_active(port: u16) -> bool {
	let state: Vec<crate::ingress::state::PortStatus> = scan_ports_config(&[]).await;
	state.iter().any(|s| s.port == port && s.active)
}

async fn is_listener_still_required(port: u16, protocol: &Protocol) -> bool {
	let current_state = CONFIG_STATE.load();
	let state: Vec<crate::ingress::state::PortStatus> = scan_ports_config(&current_state).await;
	state.iter().any(|s| {
		if s.port != port {
			return false;
		}
		match protocol {
			Protocol::Tcp => s.tcp_config.is_some(),
			Protocol::Udp => s.udp_config.is_some(),
		}
	})
}

pub async fn handle_listener_error(port: u16, protocol: Protocol, error: std::io::Error) {
	log(
		LogLevel::Warn,
		&format!(
			"⚠ Listener error on port {} {:?}: {}",
			port, protocol, error
		),
	);
	if is_listener_still_required(port, &protocol).await {
		log(
			LogLevel::Info,
			&format!(
				"↻ Retrying {:?} listener on port {} in 5s...",
				protocol, port
			),
		);
		tokio::time::sleep(std::time::Duration::from_secs(5)).await;
		start_listener(port, protocol);
	}
}
