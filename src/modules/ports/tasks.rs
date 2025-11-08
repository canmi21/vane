/* src/modules/ports/tasks.rs */

use super::model::{ListenerState, Protocol, TASK_REGISTRY};
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
				Ok((_socket, addr)) = listener.accept() => {
					if let Some(task) = TASK_REGISTRY.get(&key) {
						let mut state = task.state.lock().await;
						if let ListenerState::Draining {..} = *state {
							log(LogLevel::Debug, &format!("⚙ Rejecting new connection from {} on draining port {}", addr, port));
							continue;
						}
						*state = ListenerState::Active;
					}
					// TODO: Handle the actual proxying of the connection.
					log(LogLevel::Debug, &format!("⚙ Accepted TCP connection from {} on port {}", addr, port));
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
					// TODO: Handle the actual proxying of the UDP datagram.
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
