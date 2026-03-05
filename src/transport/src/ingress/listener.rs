/* src/ingress/listener.rs */

use super::state::{ListenerState, Protocol, RunningListener, TASK_REGISTRY};
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
		let listen_ipv6 = envflag::get::<bool>("LISTEN_IPV6", false);
		let addr: std::net::SocketAddr =
			if listen_ipv6 { ([0; 8], port).into() } else { ([0; 4], port).into() };

		log(LogLevel::Info, &format!("⚙ Binding {protocol:?} listener on {addr}..."));

		let shutdown_handle = match protocol {
			Protocol::Tcp => {
				let mut listener = None;
				for i in 0..5 {
					match TcpListener::bind(addr).await {
						Ok(l) => {
							listener = Some(l);
							break;
						}
						Err(e) => {
							if i == 4 {
								log(
									LogLevel::Error,
									&format!("✗ TCP bind failed on {addr}: {e} (giving up after 5 retries)"),
								);
							} else {
								// Retry shortly, allowing old listener time to release port
								tokio::time::sleep(std::time::Duration::from_millis(100)).await;
							}
						}
					}
				}
				listener.map(|l| super::tcp::spawn_tcp_listener_task(port, l))
			}
			Protocol::Udp => {
				let mut socket = None;
				for i in 0..5 {
					match UdpSocket::bind(addr).await {
						Ok(s) => {
							socket = Some(s);
							break;
						}
						Err(e) => {
							if i == 4 {
								log(
									LogLevel::Error,
									&format!("✗ UDP bind failed on {addr}: {e} (giving up after 5 retries)"),
								);
							} else {
								tokio::time::sleep(std::time::Duration::from_millis(100)).await;
							}
						}
					}
				}
				socket.map(|s| super::udp::spawn_udp_listener_task(port, s))
			}
		};

		if let Some(handle) = shutdown_handle {
			TASK_REGISTRY.insert(
				key,
				RunningListener { state: Arc::new(Mutex::new(ListenerState::Active)), shutdown: handle },
			);
			log(LogLevel::Info, &format!("✓ {protocol:?} listener on port {port} is UP"));
		}
	});
}

pub fn stop_listener(port: u16, protocol: Protocol) {
	if let Some((_, task)) = TASK_REGISTRY.remove(&(port, protocol)) {
		task.shutdown.shutdown();
	}
}

#[must_use]
pub fn is_port_active(port: u16) -> bool {
	let config = vane_engine::config::get();
	let port_str = port.to_string();
	config.listeners.get_tcp(&port_str).is_some() || config.listeners.get_udp(&port_str).is_some()
}

async fn is_listener_still_required(port: u16, protocol: &Protocol) -> bool {
	let config = vane_engine::config::get();
	let port_str = port.to_string();
	match protocol {
		Protocol::Tcp => config.listeners.get_tcp(&port_str).is_some(),
		Protocol::Udp => config.listeners.get_udp(&port_str).is_some(),
	}
}

pub async fn handle_listener_error(port: u16, protocol: Protocol, error: std::io::Error) {
	log(LogLevel::Warn, &format!("⚠ Listener error on port {port} {protocol:?}: {error}"));
	if is_listener_still_required(port, &protocol).await {
		log(LogLevel::Info, &format!("↻ Retrying {protocol:?} listener on port {port} in 5s..."));
		tokio::time::sleep(std::time::Duration::from_secs(5)).await;
		start_listener(port, protocol);
	}
}
