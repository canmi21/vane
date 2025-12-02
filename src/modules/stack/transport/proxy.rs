/* src/modules/stack/transport/proxy.rs */

use super::{
	balancer, context, flow, health,
	model::{DetectMethod, ResolvedTarget},
	session::{REVERSE_SESSIONS, SESSIONS, Session},
	udp::{UdpConfig, UdpDestination, UdpProtocolRule},
};
use crate::{
	common::{getenv, ip},
	modules::{
		kv::KvStore,
		plugins::model::ConnectionObject, // Needed for wrapping the socket
	},
};
use fancy_log::{LogLevel, log};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::{
	io,
	net::{TcpStream, UdpSocket},
	time::{Duration, Instant},
};

// --- TCP Logic ---

pub async fn proxy_tcp_stream(
	mut client_socket: TcpStream,
	target: ResolvedTarget,
) -> io::Result<(u64, u64)> {
	let peer_addr = client_socket
		.peer_addr()
		.map_or_else(|_| "unknown".to_string(), |a| a.to_string());
	let target_str = format!("{}:{}", target.ip, target.port);
	log(
		LogLevel::Debug,
		&format!(
			"➜ Proxying TCP connection from {} to {}",
			peer_addr, target_str
		),
	);

	match TcpStream::connect((target.ip.as_str(), target.port)).await {
		Ok(mut upstream_socket) => {
			match tokio::io::copy_bidirectional(&mut client_socket, &mut upstream_socket).await {
				Ok((up, down)) => {
					log(
						LogLevel::Debug,
						&format!(
							"✓ TCP proxy finished for {}. Upstream: {} bytes, Downstream: {} bytes.",
							peer_addr, up, down
						),
					);
					Ok((up, down))
				}
				Err(e) => {
					log(
						LogLevel::Warn,
						&format!("✗ TCP proxy error for {}: {}", peer_addr, e),
					);
					Err(e)
				}
			}
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to connect to upstream target {}: {}",
					target_str, e
				),
			);
			health::mark_tcp_target_unhealthy(&target);
			Err(e)
		}
	}
}

// --- UDP Logic ---

async fn bind_upstream_socket(target_ip: &IpAddr) -> Result<UdpSocket, std::io::Error> {
	let bind_addr: SocketAddr = if target_ip.is_ipv6() {
		([0; 16], 0).into()
	} else {
		([0; 4], 0).into()
	};
	UdpSocket::bind(bind_addr).await
}

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
						if let Some((_, _client_addr)) = REVERSE_SESSIONS.remove(&local_addr) {}
						break;
					}
				}
			}
		}
	});
}

/// Directly proxies a UDP datagram to a specific target, handling session management.
/// This is used by the Flow Engine's Transparent Proxy plugin.
///
/// It uses a simplified session key: `(client_addr, target_string)`.
pub async fn proxy_udp_direct(
	main_socket: Arc<UdpSocket>,
	datagram: &[u8],
	client_addr: SocketAddr,
	target: ResolvedTarget,
) -> io::Result<()> {
	// Construct a unique key for this flow.
	// We use the target string as the "protocol discriminator" to allow one client
	// to talk to multiple backends simultaneously.
	let discriminator = format!("flow:{}:{}", target.ip, target.port);
	let session_key = (client_addr, discriminator.clone());

	// 1. Try to find an existing healthy session
	if let Some((_, session)) = SESSIONS.remove(&session_key) {
		if health::is_udp_target_healthy(&session.target) {
			let updated_session = Arc::new(Session {
				target: session.target.clone(),
				upstream_socket: session.upstream_socket.clone(),
				last_seen: Instant::now(),
			});
			SESSIONS.insert(session_key.clone(), updated_session.clone());

			// Send data
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
				// Handle send error
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
			// Session exists but target is unhealthy; teardown and fall through to create new one
			if let Ok(addr) = session.upstream_socket.local_addr() {
				REVERSE_SESSIONS.remove(&addr);
			}
		}
	}

	// 2. Create new session
	if let Ok(target_ip) = target.ip.parse::<IpAddr>() {
		if let Ok(upstream_socket) = bind_upstream_socket(&target_ip).await {
			let upstream_arc = Arc::new(upstream_socket);
			if let Ok(local_addr) = upstream_arc.local_addr() {
				let new_session = Arc::new(Session {
					target: target.clone(),
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
				spawn_reply_handler(
					upstream_arc.clone(),
					main_socket,
					Duration::from_millis(timeout_ms),
				);

				// Send initial data
				let target_addr = (target.ip.as_str(), target.port);
				upstream_arc.send_to(datagram, target_addr).await?;

				log(
					LogLevel::Debug,
					&format!(
						"➜ Created new UDP session for {} -> {}",
						client_addr, discriminator
					),
				);
				return Ok(());
			}
		}
	}

	Err(io::Error::new(
		io::ErrorKind::Other,
		"Failed to create UDP session",
	))
}

// Internal helper for Legacy Mode
async fn get_or_create_legacy_session(
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

/// Dispatches an incoming UDP datagram.
/// Matches on config type to route to Legacy or Flow logic.
pub async fn dispatch_udp_datagram(
	socket: Arc<UdpSocket>,
	port: u16,
	config: Arc<UdpConfig>,
	datagram: Vec<u8>,
	client_addr: SocketAddr,
	mut kv_store: KvStore,
) {
	match &*config {
		UdpConfig::Legacy(legacy_config) => {
			let mut rules = legacy_config.rules.clone();
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
					if let Some(session) =
						get_or_create_legacy_session(client_addr, port, &rule, socket.clone()).await
					{
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
		UdpConfig::Flow(flow_config) => {
			log(
				LogLevel::Debug,
				&format!("⚙ Entering Flow Engine path for UDP from {}.", client_addr),
			);

			// 1. Populate Context
			context::populate_udp_context(&datagram, &mut kv_store);

			// 2. Wrap Connection Object
			let conn_object = ConnectionObject::Udp {
				socket,
				datagram,
				client_addr,
			};

			// 3. Execute Flow
			if let Err(e) = flow::execute(&flow_config.connection, &mut kv_store, conn_object).await {
				log(
					LogLevel::Error,
					&format!("✗ UDP Flow execution failed for {}: {}", client_addr, e),
				);
			}
		}
	}
}
