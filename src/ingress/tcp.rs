/* src/ingress/tcp.rs */

use super::state::{CONFIG_STATE, ListenerState, Protocol, TASK_REGISTRY};

use crate::layers::l4::dispatcher;

use crate::resources::kv;
use fancy_log::{LogLevel, log};
use tokio::{net::TcpListener, sync::oneshot};

/// Spawns a dedicated Tokio task to listen for TCP connections on a given port.
pub fn spawn_tcp_listener_task(port: u16, listener: TcpListener) -> oneshot::Sender<()> {
	let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
	let key = (port, Protocol::Tcp);
	tokio::spawn(async move {
		loop {
			tokio::select! {
				Ok((socket, addr)) = listener.accept() => {
					let client_ip: std::net::IpAddr = addr.ip();

					// Apply Connection Rate Limits
					let _guard = match super::tasks::GLOBAL_TRACKER.acquire(client_ip) {
						Some(g) => g,
						None => {
							log(LogLevel::Debug, &format!("⚙ Rate limited TCP connection from {} on port {}", addr, port));
							continue;
						}
					};

					if let Some(task) = TASK_REGISTRY.get(&key) {
						let mut state = task.state.lock().await;
						if let ListenerState::Draining {..} = *state {
							log(LogLevel::Debug, &format!("⚙ Rejecting new connection from {} on draining port {}", addr, port));
							continue;
						}
						*state = ListenerState::Active;
					}

					// Create the KV store as soon as the connection is accepted.
					let server_addr = socket.local_addr().unwrap_or_else(|_| format!("0.0.0.0:{}", port).parse().unwrap());
					let kv_store = kv::new(&addr, &server_addr, "tcp");
					log(LogLevel::Debug, &format!("⚙ Accepted TCP connection from {} on port {}", addr, port));

					let config_guard = CONFIG_STATE.load();
					let port_status = config_guard.iter().find(|s| s.port == port);
					if let Some(status) = port_status {
						if let Some(tcp_config) = status.tcp_config.clone() {
							tokio::spawn(async move {
								// Move guard into the task so it lives as long as the connection
								let _conn_guard = _guard;
								dispatcher::dispatch_tcp_connection(socket, port, tcp_config, kv_store).await;
							});
						} else {
							log(LogLevel::Warn, &format!("✗ TCP listener is active on port {}, but no config found. Dropping connection from {}.", port, addr));
						}
					}
												}
																		_ = &mut shutdown_rx => {				                    log(LogLevel::Debug, &format!("⚙ TCP listener on port {} received shutdown signal.", port));					break;
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
