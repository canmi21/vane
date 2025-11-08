/* src/modules/server/l4/dispatcher.rs */

use super::{
	balancer,
	model::{DetectMethod, TcpConfig, TcpDestination},
};
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

	// Clone the rules and sort them by priority, ascending.
	let mut rules = config.rules.clone();
	rules.sort_by_key(|r| r.priority);

	// Peek at the initial data from the socket without consuming it.
	let mut buf = [0; 64]; // A 64-byte buffer is usually enough for protocol detection.
	let n = match socket.peek(&mut buf).await {
		Ok(n) => n,
		Err(e) => {
			log(
				LogLevel::Warn,
				&format!("✗ Failed to peek initial data from {}: {}", peer_addr, e),
			);
			return; // Connection is likely dead.
		}
	};

	// If the client sends no data, we can't detect anything.
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
				// Pattern is like "0x16". We parse it to a byte.
				if let Some(hex_str) = rule.detect.pattern.strip_prefix("0x") {
					if let Ok(byte) = u8::from_str_radix(hex_str, 16) {
						incoming_data.starts_with(&[byte])
					} else {
						false // Invalid pattern
					}
				} else {
					false // Invalid pattern format
				}
			}
			DetectMethod::Prefix => {
				// Pattern is a string like "GET ".
				incoming_data.starts_with(rule.detect.pattern.as_bytes())
			}
			DetectMethod::Regex => {
				// Compile the regex. This should not fail due to pre-validation.
				if let Ok(re) = fancy_regex::Regex::new(&rule.detect.pattern) {
					// Regex works on strings, so we attempt a UTF-8 conversion.
					// This is safe because if it's not valid UTF-8, it can't match a text regex.
					if let Ok(data_str) = std::str::from_utf8(incoming_data) {
						re.is_match(data_str).unwrap_or(false)
					} else {
						false
					}
				} else {
					false
				}
			}
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
					// TODO: Hand off to the L7 resolver.
					log(
						LogLevel::Debug,
						&format!("⚙ Handing off to L7 resolver: {}", resolver),
					);
					// For now, we just close the connection.
					return;
				}
				TcpDestination::Forward { ref forward } => {
					if let Some(target) = balancer::select_target(port, &rule.name, forward) {
						// We found a target, now proxy the connection.
						// The proxy function will consume the socket and handle everything else.
						proxy_connection(socket, (target.ip, target.port)).await;
					} else {
						// No available targets were found for this rule.
						log(
							LogLevel::Warn,
							&format!(
								"✗ No available targets for protocol '{}' from {}. Dropping.",
								rule.name, peer_addr
							),
						);
					}
					return; // Stop processing further rules once one has been actioned.
				}
			}
		}
	}

	// If no rule matched after checking all of them.
	log(
		LogLevel::Warn,
		&format!(
			"✗ No protocol matched for connection from {}. Dropping connection.",
			peer_addr
		),
	);
	// Forcefully close the connection. The drop is implicit, but shutdown sends RST.
	let _ = socket.shutdown().await;
}
