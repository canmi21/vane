/* src/modules/ports/tasks.rs */

use super::model::{CONFIG_STATE, ListenerState, Protocol, TASK_REGISTRY};
use crate::modules::{
	kv,
	stack::transport::{dispatcher, udp},
};
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::{
	net::{TcpListener, UdpSocket},
	sync::oneshot,
};

/// Spawns a dedicated Tokio task to listen for TCP connections on a given port.
pub fn spawn_tcp_listener_task(port: u16, listener: TcpListener) -> oneshot::Sender<()> {
	let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
	let key = (port, Protocol::Tcp);
	tokio::spawn(async move {
		loop {
			tokio::select! {
				Ok((socket, addr)) = listener.accept() => {
					if let Some(task) = TASK_REGISTRY.get(&key) {
						let mut state = task.state.lock().await;
						if let ListenerState::Draining {..} = *state {
							log(LogLevel::Debug, &format!("⚙ Rejecting new connection from {} on draining port {}", addr, port));
							continue;
						}
						*state = ListenerState::Active;
					}

					// Create the KV store as soon as the connection is accepted.
					let kv_store = kv::new(&addr, "tcp");

					log(LogLevel::Debug, &format!("⚙ Accepted TCP connection from {} on port {}", addr, port));
					let config_guard = CONFIG_STATE.load();
					let port_status = config_guard.iter().find(|s| s.port == port);
					if let Some(status) = port_status {
						if let Some(tcp_config) = status.tcp_config.clone() {
							tokio::spawn(async move { dispatcher::dispatch_tcp_connection(socket, port, tcp_config, kv_store).await; });
						} else {
							log(LogLevel::Warn, &format!("✗ TCP listener is active on port {}, but no config found. Dropping connection from {}.", port, addr));
						}
					}
				}
				_ = &mut shutdown_rx => {
					log(LogLevel::Debug, &format!("⚙ TCP listener on port {} received shutdown signal.", port));
					break;
				}
			}
		}
		TASK_REGISTRY.remove(&key);
		log(
			LogLevel::Debug,
			&format!("⚙ TCP listener on port {} has shut down.", port),
		);
	});
	shutdown_tx
}

/// Spawns a dedicated Tokio task to handle UDP datagrams on a given port.
pub fn spawn_udp_listener_task(port: u16, socket: UdpSocket) -> oneshot::Sender<()> {
	let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
	let key = (port, Protocol::Udp);
	let socket_arc = Arc::new(socket);

	tokio::spawn(async move {
		let config_guard = CONFIG_STATE.load();
		let port_status = config_guard.iter().find(|s| s.port == port).cloned();

		if let Some(status) = port_status {
			if let Some(udp_config) = status.udp_config {
				let mut buf = vec![0u8; 65535];
				loop {
					tokio::select! {
						Ok((len, client_addr)) = socket_arc.recv_from(&mut buf) => {
							log(LogLevel::Debug, &format!("⚙ Received {} bytes via UDP from {} on port {}", len, client_addr, port));
							let datagram = buf[..len].to_vec();
							let socket_clone = socket_arc.clone();
							let config_clone = udp_config.clone();

							// Create the KV store as soon as the datagram is received.
							let kv_store = kv::new(&client_addr, "udp");

							tokio::spawn(async move {
								// FIXED: Route to the 'udp' module dispatcher which handles Flow Upgrade
								udp::dispatch_udp_datagram(socket_clone, port, config_clone, datagram, client_addr, kv_store).await;
							});
						}
						_ = &mut shutdown_rx => {
							log(LogLevel::Debug, &format!("⚙ UDP listener on port {} received shutdown signal.", port));
							break;
						}
					}
				}
			} else {
				log(
					LogLevel::Warn,
					&format!(
						"✗ UDP listener started on port {}, but no config found.",
						port
					),
				);
			}
		}

		TASK_REGISTRY.remove(&key);
		log(
			LogLevel::Debug,
			&format!("⚙ UDP listener on port {} has shut down.", port),
		);
	});

	shutdown_tx
}
