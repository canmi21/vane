/* src/modules/ports/listener.rs */

use super::{
	model::{ListenerState, Protocol, RunningListener, TASK_REGISTRY},
	tasks,
};
use crate::common::getenv;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::{
	net::{TcpListener, UdpSocket},
	time::{Duration, Instant, sleep},
};

const RETRY_DELAYS: &[u64] = &[1, 3, 5, 10, 15, 30, 60];

/// Starts a listener for a given port and protocol.
/// Spawns a task that attempts to bind the port, retrying on failure.
/// It respects the `LISTEN_IPV6` environment variable for binding.
pub fn start_listener(port: u16, protocol: Protocol) {
	let key = (port, protocol.clone());
	if TASK_REGISTRY.contains_key(&key) {
		return;
	}

	tokio::spawn(async move {
		let listen_ipv6 = getenv::get_env("LISTEN_IPV6", "false".to_string()).to_lowercase() == "true";

		let mut delay_index = 0;
		loop {
			if !is_listener_still_required(port, &protocol).await {
				let proto_str = format!("{:?}", protocol).to_uppercase();
				log(
					LogLevel::Debug,
					&format!(
						"⚙ Aborting bind retry for {} on port {}: no longer required.",
						proto_str, port
					),
				);
				return;
			}

			let bind_and_spawn_result = match protocol {
				Protocol::Tcp => {
					let bind_result = if listen_ipv6 {
						TcpListener::bind(("::", port)).await
					} else {
						TcpListener::bind(("0.0.0.0", port)).await
					};

					match bind_result {
						Ok(listener) => {
							let shutdown_tx = tasks::spawn_tcp_listener_task(port, listener);
							let task = RunningListener {
								state: Arc::new(tokio::sync::Mutex::new(ListenerState::Active)),
								shutdown_tx,
							};
							TASK_REGISTRY.insert((port, Protocol::Tcp), task);
							Ok(())
						}
						Err(e) => Err(e),
					}
				}
				Protocol::Udp => {
					let bind_result = if listen_ipv6 {
						UdpSocket::bind(("::", port)).await
					} else {
						UdpSocket::bind(("0.0.0.0", port)).await
					};

					match bind_result {
						Ok(socket) => {
							let shutdown_tx = tasks::spawn_udp_listener_task(port, socket);
							let task = RunningListener {
								state: Arc::new(tokio::sync::Mutex::new(ListenerState::Active)),
								shutdown_tx,
							};
							TASK_REGISTRY.insert((port, Protocol::Udp), task);
							Ok(())
						}
						Err(e) => Err(e),
					}
				}
			};

			if let Err(e) = bind_and_spawn_result {
				let delay = RETRY_DELAYS[delay_index.min(RETRY_DELAYS.len() - 1)];
				let proto_str = format!("{:?}", protocol).to_uppercase();
				log(
					LogLevel::Warn,
					&format!(
						"✗ Failed to bind {} on port {}: {}. Retrying in {}s...",
						proto_str, port, e, delay
					),
				);
				sleep(Duration::from_secs(delay)).await;
				if delay_index < RETRY_DELAYS.len() - 1 {
					delay_index += 1;
				}
			} else {
				return;
			}
		}
	});
}

/// Stops a listener for a given port and protocol with a graceful shutdown period.
pub fn stop_listener(port: u16, protocol: Protocol) {
	let key = (port, protocol);
	if let Some((_, task)) = TASK_REGISTRY.remove(&key) {
		tokio::spawn(async move {
			let port = key.0;
			let protocol = key.1;
			{
				let mut state = task.state.lock().await;
				*state = ListenerState::Draining {
					since: Instant::now(),
				};
			}
			let proto_str = format!("{:?}", protocol).to_uppercase();
			log(
				LogLevel::Debug,
				&format!("⚙ Draining {} listener on port {}...", proto_str, port),
			);
			let _ = task.shutdown_tx.send(());
		});
	}
}

/// Checks the config state to see if a listener is still required.
/// This is used to abort bind retries if a config is removed.
async fn is_listener_still_required(port: u16, protocol: &Protocol) -> bool {
	let state = crate::modules::ports::hotswap::scan_ports_config();
	state
		.iter()
		.any(|s| s.port == port && s.protocols.contains(protocol))
}
