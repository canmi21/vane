/* src/transport/src/l4/legacy/tcp.rs */

// Legacy type definitions now live in vane-engine
pub use vane_engine::config::{
	LegacyTcpConfig, TcpDestination, TcpProtocolRule, TcpSession, validate_tcp_rules,
};

// Dispatch function stays here (will move to vane-transport in Step 5)
use crate::l4::proxy;
use fancy_log::{LogLevel, log};
use tokio::{io::AsyncWriteExt, net::TcpStream};
use vane_engine::shared::balancer;
use vane_primitives::kv::KvStore;
use vane_primitives::model::DetectMethod;

pub async fn dispatch_legacy_tcp(
	mut socket: TcpStream,
	port: u16,
	config: &LegacyTcpConfig,
	_kv_store: KvStore,
) {
	let peer_addr = socket.peer_addr().map_or_else(|_| "unknown".to_owned(), |a| a.to_string());
	let mut rules = config.rules.clone();
	rules.sort_by_key(|r| r.priority);

	let limit = envflag::get::<usize>("TCP_DETECT_LIMIT", 64);
	const MAX_DETECT_LIMIT: usize = 8192;
	let final_limit = limit.min(MAX_DETECT_LIMIT);
	let mut buf = vec![0u8; final_limit];

	let n = match socket.peek(&mut buf).await {
		Ok(n) => n,
		Err(e) => {
			log(LogLevel::Warn, &format!("⚠ Failed to peek initial data from {peer_addr}: {e}"));
			return;
		}
	};

	if n == 0 {
		log(LogLevel::Debug, "⚙ Connection closed.");
		return;
	}

	let incoming_data = &buf[..n];
	for rule in rules {
		let matches = match &rule.detect.method {
			DetectMethod::Magic => {
				if let Some(hex_str) = rule.detect.pattern.strip_prefix("0x") {
					u8::from_str_radix(hex_str, 16).is_ok_and(|b| incoming_data.starts_with(&[b]))
				} else {
					false
				}
			}
			DetectMethod::Prefix => {
				let pattern_bytes = rule.detect.pattern.as_bytes();
				incoming_data.windows(pattern_bytes.len()).any(|window| window == pattern_bytes)
			}
			DetectMethod::Regex => {
				#[cfg(any(feature = "tcp", feature = "udp"))]
				{
					if let Ok(re) = fancy_regex::Regex::new(&rule.detect.pattern) {
						if let Ok(data_str) = std::str::from_utf8(incoming_data) {
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
			log(
				LogLevel::Info,
				&format!(
					"➜ Matched Protocol[{}] {} for connection from {}",
					rule.priority, rule.name, peer_addr
				),
			);
			match rule.destination {
				TcpDestination::Resolver { resolver } => {
					log(LogLevel::Debug, &format!("⚙ Legacy Resolver: {resolver}"));
					return;
				}
				TcpDestination::Forward { ref forward } => {
					if let Some(target) = balancer::select_tcp_target(port, &rule.name, forward).await {
						let _ = proxy::proxy_tcp_stream(socket, target).await;
					} else {
						log(LogLevel::Warn, "⚠ No available targets.");
					}
					return;
				}
			}
		}
	}
	let _ = socket.shutdown().await;
}
