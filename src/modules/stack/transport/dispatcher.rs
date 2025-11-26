/* src/modules/stack/transport/dispatcher.rs */

use super::{
	balancer, flow,
	model::DetectMethod,
	proxy,
	tcp::{LegacyTcpConfig, TcpConfig, TcpDestination},
};
use crate::{
	common::getenv,
	modules::{kv::KvStore, plugins::model::ConnectionObject},
};
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::{io::AsyncWriteExt, net::TcpStream};

/// Dispatches an incoming TCP connection.
/// It now matches on the config type to decide which execution path to take.
pub async fn dispatch_tcp_connection(
	socket: TcpStream,
	port: u16,
	config: Arc<TcpConfig>,
	mut kv_store: KvStore,
) {
	let peer_addr = socket
		.peer_addr()
		.map_or_else(|_| "unknown".to_string(), |a| a.to_string());

	match &*config {
		TcpConfig::Legacy(legacy_config) => {
			dispatch_legacy_tcp(socket, port, legacy_config, kv_store).await;
		}
		TcpConfig::Flow(flow_config) => {
			log(
				LogLevel::Debug,
				&format!(
					"⚙ Entering Flow Engine path for connection from {}.",
					peer_addr
				),
			);

			let limit_str = getenv::get_env("TCP_DETECT_LIMIT", "64".to_string());
			let limit = limit_str.parse::<usize>().unwrap_or(64);
			const MAX_DETECT_LIMIT: usize = 8192;
			let final_limit = limit.min(MAX_DETECT_LIMIT);
			let mut buf = vec![0u8; final_limit];

			match socket.peek(&mut buf).await {
				Ok(n) => {
					if n > 0 {
						let payload_hex = hex::encode(&buf[..n]);
						kv_store.insert("req.peek_buffer_hex".to_string(), payload_hex);
						kv_store.insert("conn.proto".to_string(), "tcp".to_string());

						let conn_object = ConnectionObject::Tcp(socket);

						if let Err(e) = flow::execute(&flow_config.connection, &mut kv_store, conn_object).await
						{
							log(
								LogLevel::Error,
								&format!("✗ Flow execution failed for {}: {}", peer_addr, e),
							);
						}
					} else {
						log(
							LogLevel::Debug,
							&format!(
								"⚙ Connection from {} closed before sending data (Flow path).",
								peer_addr
							),
						);
					}
				}
				Err(e) => {
					log(
						LogLevel::Warn,
						&format!(
							"✗ Failed to peek initial data from {} (Flow path): {}",
							peer_addr, e
						),
					);
				}
			}
		}
	}
}

/// The original dispatch logic, refactored into its own function for the legacy path.
async fn dispatch_legacy_tcp(
	mut socket: TcpStream,
	port: u16,
	config: &LegacyTcpConfig,
	_kv_store: KvStore,
) {
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
					// TODO: Implement the resolver handoff logic.
					return;
				}
				TcpDestination::Forward { ref forward } => {
					if let Some(target) = balancer::select_tcp_target(port, &rule.name, forward).await {
						let _ = proxy::proxy_tcp_stream(socket, target).await;
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
