/* src/layers/l4/legacy/udp.rs */

use crate::common::{config::env_loader, net::ip};
use crate::ingress::tasks::GLOBAL_TRACKER;
use crate::layers::l4::model::{DetectMethod, Forward};
use crate::layers::l4::session::{REVERSE_SESSIONS, SESSIONS, Session};
use crate::layers::l4::{balancer, health};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{Duration, Instant};
use validator::{Validate, ValidationError, ValidationErrors};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UdpDestination {
	Resolver { resolver: String },
	Forward { forward: Forward },
}

impl Validate for UdpDestination {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			UdpDestination::Resolver { .. } => Ok(()),
			UdpDestination::Forward { forward } => forward.validate(),
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct UdpProtocolRule {
	#[validate(regex(
        path = *crate::layers::l4::model::NAME_REGEX,
        message = "can only contain lowercase letters and numbers"
    ))]
	pub name: String,
	#[validate(range(min = 1))]
	pub priority: u32,
	#[validate(nested)]
	pub detect: crate::layers::l4::model::Detect,
	#[validate(nested)]
	pub destination: UdpDestination,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Validate)]
pub struct LegacyUdpConfig {
	#[serde(rename = "protocols")]
	#[validate(nested)]
	pub rules: Vec<UdpProtocolRule>,
}

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
			if current_session.is_none() {
				match &rule.destination {
					UdpDestination::Forward { forward } => {
						if let Some(target) = balancer::select_udp_target(port, &rule.name, forward).await {
							if let Ok(target_ip) = target.ip.parse::<IpAddr>() {
								if let Ok(upstream_socket) = bind_upstream_socket(&target_ip).await {
									let upstream_arc = Arc::new(upstream_socket);
									if let Ok(local_addr) = upstream_arc.local_addr() {
										// Apply Connection Rate Limits
										let guard = match GLOBAL_TRACKER.acquire(client_addr.ip()) {
											Some(g) => g,
											None => {
												log(
													LogLevel::Debug,
													&format!(
														"⚙ Rate limited UDP session from {} for rule {}",
														client_addr, rule.name
													),
												);
												continue;
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
											env_loader::get_env("UDP_TIMEOUT_LOCAL", "500".to_string())
										} else {
											env_loader::get_env("UDP_TIMEOUT_REMOTE", "5000".to_string())
										};
										let timeout_ms = timeout_ms_str.parse::<u64>().unwrap_or(5000);

										spawn_reply_handler(
											upstream_arc,
											socket.clone(),
											Duration::from_millis(timeout_ms),
										);

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
							}
						}
					}
					_ => {}
				}
			}

			// 3. Send Data
			if let Some(session) = current_session {
				let target_addr = (session.target.ip.as_str(), session.target.port);
				if session
					.upstream_socket
					.send_to(datagram, target_addr)
					.await
					.is_err()
				{
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

pub fn validate_udp_rules(rules: &[UdpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
	}
	Ok(())
}
