/* src/modules/server/l4/dispatcher.rs */

use super::model::{DetectMethod, TcpConfig, TcpDestination};
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::{io::AsyncWriteExt, net::TcpStream};

/// Dispatches an incoming TCP connection based on the listener's configuration.
pub async fn dispatch_tcp_connection(mut socket: TcpStream, config: Arc<TcpConfig>) {
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
		};

		if matches {
			// MODIFIED: Updated log format as per the new request.
			log(
				LogLevel::Info,
				&format!(
					"⇅ Matched Protocol[{:}] {} for connection from {}",
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
				TcpDestination::Forward { forward: _ } => {
					// TODO: Implement the L4 forwarding logic.
					log(
						LogLevel::Debug,
						"⚙ Destination is 'forward'. Proxy logic to be implemented.",
					);
					// For now, we just close the connection.
					return;
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
