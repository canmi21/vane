/* src/layers/l4/proxy/udp.rs */

use crate::common::{config::getenv, net::ip};
use crate::layers::l4::{
	health,
	model::ResolvedTarget,
	session::{REVERSE_SESSIONS, SESSIONS, Session},
};
use fancy_log::{LogLevel, log};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::{
	io,
	net::UdpSocket,
	time::{Duration, Instant},
};

pub async fn bind_upstream_socket(target_ip: &IpAddr) -> io::Result<UdpSocket> {
	let bind_addr: SocketAddr = if target_ip.is_ipv6() {
		([0; 16], 0).into()
	} else {
		([0; 4], 0).into()
	};
	UdpSocket::bind(bind_addr).await
}

pub fn spawn_reply_handler(
	upstream_socket: Arc<UdpSocket>,
	main_socket: Arc<UdpSocket>,
	timeout: Duration,
) {
	tokio::spawn(async move {
		let mut buf = [0; 65535];
		if let Ok(local_addr) = upstream_socket.local_addr() {
			loop {
				match tokio::time::timeout(timeout, upstream_socket.recv_from(&mut buf)).await {
					Ok(Ok((len, _))) => {
						if let Some(client_addr) = REVERSE_SESSIONS.get(&local_addr) {
							if main_socket
								.send_to(&buf[..len], *client_addr)
								.await
								.is_err()
							{
								break;
							}
						}
					}
					_ => {
						if let Some((_, _client_addr)) = REVERSE_SESSIONS.remove(&local_addr) {}
						break;
					}
				}
			}
		}
	});
}

pub async fn proxy_udp_direct(
	main_socket: Arc<UdpSocket>,
	datagram: &bytes::Bytes,
	client_addr: SocketAddr,
	target: ResolvedTarget,
) -> io::Result<()> {
	// Flow Engine / Plugin Logic
	let nat_key = format!("flow:{}:{}", target.ip, target.port);
	let session_key = (client_addr, nat_key.clone());

	if let Some((_, session)) = SESSIONS.remove(&session_key) {
		if health::is_udp_target_healthy(&session.target) {
			let updated_session = Arc::new(Session {
				target: session.target.clone(),
				upstream_socket: session.upstream_socket.clone(),
				last_seen: Instant::now(),
				_guard: session._guard.clone(),
			});
			SESSIONS.insert(session_key.clone(), updated_session.clone());

			let target_addr = (
				updated_session.target.ip.as_str(),
				updated_session.target.port,
			);
			if updated_session
				.upstream_socket
				.send_to(datagram, target_addr)
				.await
				.is_err()
			{
				health::mark_udp_target_unhealthy(&updated_session.target);
				if let Ok(addr) = updated_session.upstream_socket.local_addr() {
					REVERSE_SESSIONS.remove(&addr);
				}
				SESSIONS.remove(&session_key);
				return Err(io::Error::new(
					io::ErrorKind::ConnectionReset,
					"Failed to send to upstream",
				));
			}
			return Ok(());
		} else {
			if let Ok(addr) = session.upstream_socket.local_addr() {
				REVERSE_SESSIONS.remove(&addr);
			}
		}
	}

	if let Ok(target_ip) = target.ip.parse::<IpAddr>() {
		if let Ok(upstream_socket) = bind_upstream_socket(&target_ip).await {
			let upstream_arc = Arc::new(upstream_socket);

			if let Ok(local_addr) = upstream_arc.local_addr() {
				// Apply Connection Rate Limits
				let guard = match crate::ingress::tasks::GLOBAL_TRACKER.acquire(client_addr.ip()) {
					Some(g) => g,
					None => {
						log(
							LogLevel::Debug,
							&format!(
								"⚙ Rate limited UDP Flow session from {} to {}:{}",
								client_addr, target.ip, target.port
							),
						);
						return Err(io::Error::new(io::ErrorKind::Other, "Rate limited"));
					}
				};

				let new_session = Arc::new(Session {
					target: target.clone(),
					upstream_socket: upstream_arc.clone(),
					last_seen: Instant::now(),
					_guard: guard,
				});

				SESSIONS.insert(session_key.clone(), new_session.clone());
				REVERSE_SESSIONS.insert(local_addr, client_addr);

				let timeout_ms_str = if ip::is_private_ip(&target_ip) {
					getenv::get_env("UDP_TIMEOUT_LOCAL", "500".to_string())
				} else {
					getenv::get_env("UDP_TIMEOUT_REMOTE", "5000".to_string())
				};
				let timeout_ms = timeout_ms_str.parse::<u64>().unwrap_or(5000);

				spawn_reply_handler(
					upstream_arc.clone(),
					main_socket,
					Duration::from_millis(timeout_ms),
				);

				let target_addr = (target.ip.as_str(), target.port);

				if let Err(e) = upstream_arc.send_to(datagram, target_addr).await {
					log(
						LogLevel::Error,
						&format!(
							"✗ Failed to send initial UDP packet to {}: {}",
							target_addr.0, e
						),
					);
					health::mark_udp_target_unhealthy(&target);
					// Cleanup
					SESSIONS.remove(&session_key);
					REVERSE_SESSIONS.remove(&local_addr);
					return Err(e);
				}

				log(
					LogLevel::Debug,
					&format!(
						"➜ Established UDP NAT mapping: {} <-> {}",
						client_addr, nat_key
					),
				);
				return Ok(());
			}
		}
	}

	Err(io::Error::new(
		io::ErrorKind::Other,
		"Failed to create UDP NAT mapping",
	))
}
