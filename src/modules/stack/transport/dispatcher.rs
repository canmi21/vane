/* src/modules/stack/transport/dispatcher.rs */

use super::{
	balancer,
	model::{DetectMethod, TcpConfig, TcpDestination},
};
use crate::common::getenv;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::{io::AsyncWriteExt, net::TcpStream};

/// Forwards a TCP stream to a chosen upstream target, copying data in both directions.
async fn proxy_connection(mut client_socket: TcpStream, target_addr: (String, u16)) {
	let peer_addr = client_socket
		.peer_addr()
		.map_or_else(|_| "unknown".to_string(), |a| a.to_string());
	let target_str = format!("{}:{}", target_addr.0, target_addr.1);

	log(
		LogLevel::Debug,
		&format!("➜ Proxying connection from {} to {}", peer_addr, target_str),
	);

	match TcpStream::connect(target_addr).await {
		Ok(mut upstream_socket) => {
			match tokio::io::copy_bidirectional(&mut client_socket, &mut upstream_socket).await {
				Ok((up, down)) => log(
					LogLevel::Debug,
					&format!(
						"✓ Proxy finished for {}. Upstream: {} bytes, Downstream: {} bytes.",
						peer_addr, up, down
					),
				),
				Err(e) => log(
					LogLevel::Warn,
					&format!("✗ Proxy error for {}: {}", peer_addr, e),
				),
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
		}
	}
}

/// Dispatches an incoming TCP connection based on the listener's configuration.
pub async fn dispatch_tcp_connection(mut socket: TcpStream, port: u16, config: Arc<TcpConfig>) {
	let peer_addr = socket
		.peer_addr()
		.map_or_else(|_| "unknown".to_string(), |a| a.to_string());

	let mut rules = config.rules.clone();
	rules.sort_by_key(|r| r.priority);

	let limit_str = getenv::get_env("TCP_DETECT_LIMIT", "64".to_string());
	let limit = limit_str.parse::<usize>().unwrap_or(64);
	const MAX_DETECT_LIMIT: usize = 8192;
	let final_limit = limit.min(MAX_DETECT_LIMIT);

	let mut buf = vec![0u8; final_limit];
	let n = match socket.peek(&mut buf).await {
		Ok(n) => n,
		Err(e) => {
			log(
				LogLevel::Warn,
				&format!("✗ Failed to peek initial data from {}: {}", peer_addr, e),
			);
			return;
		}
	};

	if n == 0 {
		log(
			LogLevel::Debug,
			&format!(
				"⚙ Connection from {} closed before sending data.",
				peer_addr
			),
		);
		return;
	}

	let incoming_data = &buf[..n];

	for rule in rules {
		let matches = match &rule.detect.method {
			DetectMethod::Magic => {
				if let Some(hex_str) = rule.detect.pattern.strip_prefix("0x") {
					u8::from_str_radix(hex_str, 16).map_or(false, |b| incoming_data.starts_with(&[b]))
				} else {
					false
				}
			}
			DetectMethod::Prefix => {
				let pattern_bytes = rule.detect.pattern.as_bytes();
				incoming_data
					.windows(pattern_bytes.len())
					.any(|window| window == pattern_bytes)
			}
			DetectMethod::Regex => {
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
			DetectMethod::Fallback => true,
		};

		if matches {
			log(
				LogLevel::Info,
				&format!(
					"⇅ Matched Protocol[{}] {} for connection from {}",
					rule.priority, rule.name, peer_addr
				),
			);

			match rule.destination {
				TcpDestination::Resolver { resolver } => {
					log(
						LogLevel::Debug,
						&format!("⚙ Handing off to L7 resolver: {}", resolver),
					);
					return;
				}
				TcpDestination::Forward { ref forward } => {
					if let Some(target) = balancer::select_tcp_target(port, &rule.name, forward) {
						proxy_connection(socket, (target.ip, target.port)).await;
					} else {
						log(
							LogLevel::Warn,
							&format!(
								"✗ No available targets for protocol '{}' from {}. Dropping.",
								rule.name, peer_addr
							),
						);
					}
					return;
				}
			}
		}
	}

	log(
		LogLevel::Warn,
		&format!(
			"✗ No protocol matched for connection from {}. Dropping connection.",
			peer_addr
		),
	);
	let _ = socket.shutdown().await;
}
