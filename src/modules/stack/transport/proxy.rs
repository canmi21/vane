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

// --- TCP Logic (Unchanged) ---

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
							// Check if the main socket is still valid/open is handled by send_to failure
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
						// Timeout or error, clean up the reverse mapping
						if let Some((_, _client_addr)) = REVERSE_SESSIONS.remove(&local_addr) {}
						break;
					}
				}
			}
		}
	});
}

/// Directly proxies a UDP datagram to a specific target.
///
/// **Design Note for Flow Engine:**
/// This function treats the "Session" purely as a **NAT Mapping** for return traffic.
/// It does NOT imply a sticky logic bypass. Every packet calls this function anew.
///
/// - If a mapping exists (re-using an upstream socket), it is used for performance and to receive replies.
/// - If the Flow Engine changes the target for the next packet, a NEW mapping (socket) is created,
///   completely independent of the previous one.
pub async fn proxy_udp_direct(
	main_socket: Arc<UdpSocket>,
	datagram: &[u8],
	client_addr: SocketAddr,
	target: ResolvedTarget,
) -> io::Result<()> {
	// We create a unique key combining Client + Target.
	// This ensures that if the Flow switches the target for the same client,
	// we use a different socket/mapping.
	let nat_key = format!("flow:{}:{}", target.ip, target.port);
	let session_key = (client_addr, nat_key.clone());

	// 1. Check for existing NAT mapping (Upstream Socket Reuse)
	if let Some((_, session)) = SESSIONS.remove(&session_key) {
		if health::is_udp_target_healthy(&session.target) {
			// Update activity timestamp to prevent premature cleanup
			let updated_session = Arc::new(Session {
				target: session.target.clone(),
				upstream_socket: session.upstream_socket.clone(),
				last_seen: Instant::now(),
			});
			SESSIONS.insert(session_key.clone(), updated_session.clone());

			// Forward the datagram using the existing upstream socket
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
			// Target is unhealthy, drop this mapping and try to create a new one
			if let Ok(addr) = session.upstream_socket.local_addr() {
				REVERSE_SESSIONS.remove(&addr);
			}
		}
	}

	// 2. Create new NAT mapping (New Upstream Socket)
	if let Ok(target_ip) = target.ip.parse::<IpAddr>() {
		if let Ok(upstream_socket) = bind_upstream_socket(&target_ip).await {
			let upstream_arc = Arc::new(upstream_socket);

			if let Ok(local_addr) = upstream_arc.local_addr() {
				let new_session = Arc::new(Session {
					target: target.clone(),
					upstream_socket: upstream_arc.clone(),
					last_seen: Instant::now(),
				});

				// Store mappings
				SESSIONS.insert(session_key, new_session.clone());
				REVERSE_SESSIONS.insert(local_addr, client_addr);

				// Determine timeout (NAT TTL)
				let timeout_ms_str = if ip::is_private_ip(&target_ip) {
					getenv::get_env("UDP_TIMEOUT_LOCAL", "500".to_string())
				} else {
					getenv::get_env("UDP_TIMEOUT_REMOTE", "5000".to_string())
				};
				let timeout_ms = timeout_ms_str.parse::<u64>().unwrap_or(5000);

				// Spawn background task to handle return traffic (Back-to-Client)
				spawn_reply_handler(
					upstream_arc.clone(),
					main_socket,
					Duration::from_millis(timeout_ms),
				);

				// Send the initial datagram
				let target_addr = (target.ip.as_str(), target.port);
				upstream_arc.send_to(datagram, target_addr).await?;

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

// Internal helper for Legacy Mode (Stickiness is strictly enforced here)
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
///
/// - Legacy Mode: Uses `get_or_create_legacy_session` (Sticky Logic).
/// - Flow Mode: ALWAYS executes the full Flow pipeline for EVERY packet.
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

			// 1. Populate Context (Extract payload for inspection)
			context::populate_udp_context(&datagram, &mut kv_store);

			// 2. Wrap Connection Object
			let conn_object = ConnectionObject::Udp {
				socket,
				datagram,
				client_addr,
			};

			// 3. Execute Flow
			// This happens for EVERY packet. No session stickiness bypasses this.
			if let Err(e) = flow::execute(&flow_config.connection, &mut kv_store, conn_object).await {
				log(
					LogLevel::Error,
					&format!("✗ UDP Flow execution failed for {}: {}", client_addr, e),
				);
			}
		}
	}
}
