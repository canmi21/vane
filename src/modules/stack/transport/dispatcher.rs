/* src/modules/stack/transport/dispatcher.rs */

use super::{
	balancer, context, flow,
	model::DetectMethod,
	proxy,
	tcp::{LegacyTcpConfig, TcpConfig, TcpDestination},
};
use crate::{
	common::getenv,
	modules::{
		kv::KvStore, plugins::model::ConnectionObject, plugins::model::TerminatorResult,
		stack::protocol::carrier,
	},
};
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::{io::AsyncWriteExt, net::TcpStream};

pub async fn dispatch_tcp_connection(
	mut socket: TcpStream,
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

			match context::populate_tcp_context(&mut socket, &mut kv_store).await {
				Ok(n) => {
					if n > 0 {
						let conn_object = ConnectionObject::Tcp(socket);
						let result = flow::execute(&flow_config.connection, &mut kv_store, conn_object).await;

						match result {
							Ok(TerminatorResult::Finished) => {
								log(LogLevel::Debug, "✓ Connection handled at L4.");
							}
							Ok(TerminatorResult::Upgrade {
								protocol,
								conn,
								parent_path,
							}) => {
								log(
									LogLevel::Info,
									&format!("➜ Upgrading connection to: {}", protocol),
								);
								match (protocol.as_str(), conn) {
									("tls", ConnectionObject::Tcp(stream)) => {
										tokio::spawn(async move {
											if let Err(e) = carrier::tls::run(stream, &mut kv_store, parent_path).await {
												log(LogLevel::Error, &format!("✗ TLS Carrier failed: {:#}", e));
											}
										});
									}
									("http", ConnectionObject::Tcp(stream)) => {
										tokio::spawn(async move {
											if let Err(e) =
												carrier::plain::run(stream, &mut kv_store, parent_path, "http").await
											{
												log(LogLevel::Error, &format!("✗ HTTP Carrier failed: {:#}", e));
											}
										});
									}
									// FIXED: Create an owned String for the closure to capture
									(proto_str, ConnectionObject::Tcp(stream)) => {
										let proto_owned = proto_str.to_string();
										tokio::spawn(async move {
											if let Err(e) =
												carrier::plain::run(stream, &mut kv_store, parent_path, &proto_owned).await
											{
												log(
													LogLevel::Error,
													&format!("✗ Plain Carrier ({}) failed: {:#}", proto_owned, e),
												);
											}
										});
									}
									(p, _) => {
										log(
											LogLevel::Error,
											&format!("✗ Unsupported upgrade protocol '{}' or object mismatch.", p),
										);
									}
								}
							}
							Err(e) => {
								log(LogLevel::Error, &format!("✗ Flow execution failed: {}", e));
							}
						}
					} else {
						log(LogLevel::Debug, "⚙ Connection closed before data.");
					}
				}
				Err(e) => {
					log(LogLevel::Warn, &format!("✗ Failed to peek: {}", e));
				}
			}
		}
	}
}

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
		log(LogLevel::Debug, "⚙ Connection closed.");
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
					log(LogLevel::Debug, &format!("⚙ Legacy Resolver: {}", resolver));
					// legacy resolver placeholder
					return;
				}
				TcpDestination::Forward { ref forward } => {
					if let Some(target) = balancer::select_tcp_target(port, &rule.name, forward).await {
						let _ = proxy::proxy_tcp_stream(socket, target).await;
					} else {
						log(LogLevel::Warn, "✗ No available targets.");
					}
					return;
				}
			}
		}
	}
	let _ = socket.shutdown().await;
}
