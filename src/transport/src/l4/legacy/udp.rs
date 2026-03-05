/* src/transport/src/l4/legacy/udp.rs */

// Legacy type definitions now live in vane-engine
pub use vane_engine::config::{
	LegacyUdpConfig, UdpDestination, UdpProtocolRule, validate_udp_rules,
};

// Dispatch function stays here (will move to vane-transport in Step 5)
use fancy_log::{LogLevel, log};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{Duration, Instant};
use vane_engine::shared::balancer;
use vane_engine::shared::health;
use vane_engine::shared::session::{REVERSE_SESSIONS, SESSIONS, Session};
use vane_primitives::common::net::ip;
use vane_primitives::model::DetectMethod;
use vane_primitives::tasks::GLOBAL_TRACKER;

async fn bind_upstream_socket(target_ip: &IpAddr) -> Result<UdpSocket, std::io::Error> {
	let bind_addr: SocketAddr =
		if target_ip.is_ipv6() { ([0; 16], 0).into() } else { ([0; 4], 0).into() };
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
				if let Ok(Ok((len, _))) =
					tokio::time::timeout(timeout, upstream_socket.recv_from(&mut buf)).await
				{
					if let Some(client_addr) = REVERSE_SESSIONS.get(&local_addr)
						&& main_socket.send_to(&buf[..len], *client_addr).await.is_err()
					{
						break;
					}
				} else {
					if let Some((_, _client_addr)) = REVERSE_SESSIONS.remove(&local_addr) {}
					break;
				}
			}
		}
	});
}

pub async fn dispatch_legacy_udp(
	socket: Arc<UdpSocket>,
	port: u16,
	legacy_config: &LegacyUdpConfig,
	datagram: &[u8],
	client_addr: SocketAddr,
) {
	let mut rules = legacy_config.rules.clone();
	rules.sort_by_key(|r| r.priority);

	for rule in rules {
		let matches = match &rule.detect.method {
			DetectMethod::Magic => {
				if let Some(hex_str) = rule.detect.pattern.strip_prefix("0x") {
					u8::from_str_radix(hex_str, 16).is_ok_and(|b| datagram.starts_with(&[b]))
				} else {
					false
				}
			}
			DetectMethod::Prefix => {
				let pattern_bytes = rule.detect.pattern.as_bytes();
				datagram.windows(pattern_bytes.len()).any(|window| window == pattern_bytes)
			}
			DetectMethod::Regex => {
				#[cfg(any(feature = "tcp", feature = "udp"))]
				{
					if let Ok(re) = fancy_regex::Regex::new(&rule.detect.pattern) {
						if let Ok(data_str) = std::str::from_utf8(datagram) {
							re.is_match(data_str).unwrap_or(false)
						} else {
							false
						}
					} else {
						false
					}
				}
				#[cfg(not(any(feature = "tcp", feature = "udp")))]
				false
			}
			DetectMethod::Fallback => true,
		};

		if matches {
			// --- Legacy Session Logic ---
			let session_key = (client_addr, rule.name.clone());
			let mut current_session: Option<Arc<Session>> = None;

			// 1. Try to retrieve existing session
			if let Some((_, session)) = SESSIONS.remove(&session_key) {
				if health::is_udp_target_healthy(&session.target) {
					// Target healthy: Refresh timestamp and reuse
					let updated_session = Arc::new(Session {
						target: session.target.clone(),
						upstream_socket: session.upstream_socket.clone(),
						last_seen: Instant::now(),
						_guard: session._guard.clone(),
					});
					SESSIONS.insert(session_key.clone(), updated_session.clone());
					current_session = Some(updated_session);
				} else {
					// Target unhealthy: Cleanup reverse mapping
					if let Ok(addr) = session.upstream_socket.local_addr() {
						REVERSE_SESSIONS.remove(&addr);
					}
				}
			}

			// 2. Create new session if needed
			if current_session.is_none()
				&& let UdpDestination::Forward { forward } = &rule.destination
				&& let Some(target) = balancer::select_udp_target(port, &rule.name, forward).await
				&& let Ok(target_ip) = target.ip.parse::<IpAddr>()
				&& let Ok(upstream_socket) = bind_upstream_socket(&target_ip).await
			{
				let upstream_arc = Arc::new(upstream_socket);
				if let Ok(local_addr) = upstream_arc.local_addr() {
					// Apply Connection Rate Limits
					let Some(guard) = GLOBAL_TRACKER.acquire(client_addr.ip()) else {
						log(
							LogLevel::Debug,
							&format!("⚙ Rate limited UDP session from {} for rule {}", client_addr, rule.name),
						);
						continue;
					};

					let new_session = Arc::new(Session {
						target: target.clone(),
						upstream_socket: upstream_arc.clone(),
						last_seen: Instant::now(),
						_guard: guard,
					});
					SESSIONS.insert(session_key.clone(), new_session.clone());
					REVERSE_SESSIONS.insert(local_addr, client_addr);

					let timeout_ms = if ip::is_private_ip(&target_ip) {
						envflag::get::<u64>("UDP_TIMEOUT_LOCAL", 500)
					} else {
						envflag::get::<u64>("UDP_TIMEOUT_REMOTE", 5000)
					};

					spawn_reply_handler(upstream_arc, socket.clone(), Duration::from_millis(timeout_ms));

					log(
						LogLevel::Debug,
						&format!(
							"➜ Established Legacy UDP NAT: {} <-> {}:{}",
							client_addr, target.ip, target.port
						),
					);

					current_session = Some(new_session);
				}
			}

			// 3. Send Data
			if let Some(session) = current_session {
				let target_addr = (session.target.ip.as_str(), session.target.port);
				if session.upstream_socket.send_to(datagram, target_addr).await.is_err() {
					health::mark_udp_target_unhealthy(&session.target);
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
