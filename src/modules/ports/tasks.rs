/* src/modules/ports/tasks.rs */

use super::model::{CONFIG_STATE, ListenerState, Protocol, TASK_REGISTRY};
use crate::modules::server::l4::dispatcher;
use fancy_log::{LogLevel, log};
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
				// MODIFIED: Renamed _socket to socket to use it.
				Ok((socket, addr)) = listener.accept() => {
					if let Some(task) = TASK_REGISTRY.get(&key) {
						let mut state = task.state.lock().await;
						if let ListenerState::Draining {..} = *state {
							log(LogLevel::Debug, &format!("⚙ Rejecting new connection from {} on draining port {}", addr, port));
							continue;
						}
						*state = ListenerState::Active;
					}

					log(LogLevel::Debug, &format!("⚙ Accepted TCP connection from {} on port {}", addr, port));

					// Get the current configuration for this port.
					let config_guard = CONFIG_STATE.load();
					let port_status = config_guard.iter().find(|s| s.port == port);

					if let Some(status) = port_status {
						if let Some(tcp_config) = status.tcp_config.clone() {
							// Spawn a new task to handle the connection dispatching.
							tokio::spawn(async move {
								dispatcher::dispatch_tcp_connection(socket, tcp_config).await;
							});
						} else {
							// This should not happen if the listener is up, but as a safeguard:
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

/// Spawns a dedicated Tokio task to listen for UDP datagrams on a given port.
pub fn spawn_udp_listener_task(port: u16, socket: UdpSocket) -> oneshot::Sender<()> {
	let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
	let key = (port, Protocol::Udp);

	tokio::spawn(async move {
		let mut buf = [0; 1024];
		loop {
			tokio::select! {
				Ok((len, addr)) = socket.recv_from(&mut buf) => {
					// TODO: Handle the actual proxying of the UDP datagram using a similar dispatcher pattern.
					log(LogLevel::Debug, &format!("⚙ Received {} bytes via UDP from {} on port {}", len, addr, port));
				}
				_ = &mut shutdown_rx => {
					log(LogLevel::Debug, &format!("⚙ UDP listener on port {} received shutdown signal.", port));
					break;
				}
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
