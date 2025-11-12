/* src/modules/stack/transport/proxy.rs */

use super::{
	balancer, health,
	model::DetectMethod,
	session::{REVERSE_SESSIONS, SESSIONS, Session},
	udp::{UdpConfig, UdpDestination, UdpProtocolRule},
};
use crate::common::{getenv, ip};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::{
	net::UdpSocket,
	time::{Duration, Instant},
};

/// Binds a new UDP socket for proxying to an upstream target.
async fn bind_upstream_socket(target_ip: &IpAddr) -> Result<UdpSocket, std::io::Error> {
	let bind_addr: SocketAddr = if target_ip.is_ipv6() {
		([0; 16], 0).into()
	} else {
		([0; 4], 0).into()
	};
	UdpSocket::bind(bind_addr).await
}

/// Spawns a task to handle replies from an upstream socket back to the client.
fn spawn_reply_handler(
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
						if let Some((_, _client_addr)) = REVERSE_SESSIONS.remove(&local_addr) {
							// Note: We can't easily clean the primary SESSIONS map here
							// because we don't know the protocol name. The main cleanup
							// task will handle the dangling session entry.
						}
						break;
					}
				}
			}
		}
	});
}

/// Gets an existing session or creates a new one for a client, based on the matched protocol rule.
async fn get_or_create_session(
	client_addr: SocketAddr,
	port: u16,
	rule: &UdpProtocolRule,
	main_socket: Arc<UdpSocket>,
) -> Option<Arc<Session>> {
	let session_key = (client_addr, rule.name.clone());

	if let Some((_, session)) = SESSIONS.remove(&session_key) {
		if health::is_udp_target_healthy(&session.target) {
			let updated_session = Arc::new(Session {
				target: session.target.clone(),
				upstream_socket: session.upstream_socket.clone(),
				last_seen: Instant::now(),
			});
			SESSIONS.insert(session_key, updated_session.clone());
			return Some(updated_session);
		} else {
			if let Ok(addr) = session.upstream_socket.local_addr() {
				REVERSE_SESSIONS.remove(&addr);
			}
		}
	}

	if let UdpDestination::Forward { ref forward } = rule.destination {
		if let Some(target) = balancer::select_udp_target(port, &rule.name, forward).await {
			if let Ok(target_ip) = target.ip.parse() {
				if let Ok(upstream_socket) = bind_upstream_socket(&target_ip).await {
					let upstream_arc = Arc::new(upstream_socket);
					if let Ok(local_addr) = upstream_arc.local_addr() {
						let new_session = Arc::new(Session {
							target,
							upstream_socket: upstream_arc.clone(),
							last_seen: Instant::now(),
						});
						SESSIONS.insert(session_key, new_session.clone());
						REVERSE_SESSIONS.insert(local_addr, client_addr);

						let timeout_ms_str = if ip::is_private_ip(&target_ip) {
							getenv::get_env("UDP_TIMEOUT_LOCAL", "500".to_string())
						} else {
							getenv::get_env("UDP_TIMEOUT_REMOTE", "5000".to_string())
						};
						let timeout_ms = timeout_ms_str.parse::<u64>().unwrap_or(5000);
						spawn_reply_handler(upstream_arc, main_socket, Duration::from_millis(timeout_ms));
						return Some(new_session);
					}
				}
			}
		}
	}
	None
}

/// Dispatches a single incoming UDP datagram.
pub async fn dispatch_udp_datagram(
	socket: Arc<UdpSocket>,
	port: u16,
	config: Arc<UdpConfig>,
	datagram: Vec<u8>,
	client_addr: SocketAddr,
) {
	let mut rules = config.rules.clone();
	rules.sort_by_key(|r| r.priority);

	for rule in rules {
		let matches = match &rule.detect.method {
			DetectMethod::Magic => {
				if let Some(hex_str) = rule.detect.pattern.strip_prefix("0x") {
					u8::from_str_radix(hex_str, 16).map_or(false, |b| datagram.starts_with(&[b]))
				} else {
					false
				}
			}
			DetectMethod::Prefix => {
				let pattern_bytes = rule.detect.pattern.as_bytes();
				datagram
					.windows(pattern_bytes.len())
					.any(|window| window == pattern_bytes)
			}
			DetectMethod::Regex => {
				if let Ok(re) = fancy_regex::Regex::new(&rule.detect.pattern) {
					if let Ok(data_str) = std::str::from_utf8(&datagram) {
						re.is_match(data_str).unwrap_or(false)
					} else {
						false
					}
				} else {
					false
				}
			}
			DetectMethod::Fallback => true,
		};

		if matches {
			if let Some(session) = get_or_create_session(client_addr, port, &rule, socket.clone()).await {
				let target_addr = (session.target.ip.as_str(), session.target.port);
				if session
					.upstream_socket
					.send_to(&datagram, target_addr)
					.await
					.is_err()
				{
					health::mark_udp_target_unhealthy(&session.target);
					let session_key = (client_addr, rule.name.clone());
					if let Ok(addr) = session.upstream_socket.local_addr() {
						REVERSE_SESSIONS.remove(&addr);
					}
					SESSIONS.remove(&session_key);
				}
			}
			return;
		}
	}
}
