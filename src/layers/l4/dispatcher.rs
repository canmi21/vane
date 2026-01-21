/* src/layers/l4/dispatcher.rs */

use super::{context, flow, legacy, tcp::TcpConfig};
use crate::engine::interfaces::{ConnectionObject, TerminatorResult};

use crate::layers::l4p::{plain, tls};
use crate::resources::kv::KvStore;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::net::TcpStream;

pub async fn dispatch_tcp_connection(
	mut socket: TcpStream,
	port: u16,
	config: Arc<TcpConfig>,
	mut kv_store: KvStore,
) {
	let peer_addr = socket
		.peer_addr()
		.map_or_else(|_| "unknown".to_owned(), |a| a.to_string());

	match &*config {
		TcpConfig::Legacy(legacy_config) => {
			legacy::dispatch_legacy_tcp(socket, port, legacy_config, kv_store).await;
		}
		TcpConfig::Flow(flow_config) => {
			log(
				LogLevel::Debug,
				&format!("⚙ Entering Flow Engine path for connection from {peer_addr}."),
			);

			match context::populate_tcp_context(&mut socket, &mut kv_store).await {
				Ok(n) => {
					if n > 0 {
						let conn_object = ConnectionObject::Tcp(socket);
						let result = flow::execute(
							&flow_config.connection,
							&mut kv_store,
							conn_object,
							ahash::AHashMap::new(),
						)
						.await;

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
									&format!("➜ Upgrading connection to: {protocol}"),
								);
								match (protocol.as_str(), conn) {
									#[cfg(feature = "tls")]
									("tls", ConnectionObject::Tcp(stream)) => {
										tokio::spawn(async move {
											if let Err(e) = tls::run(stream, &mut kv_store, parent_path).await {
												log(LogLevel::Error, &format!("✗ TLS Carrier failed: {e:#}"));
											}
										});
									}
									#[cfg(not(feature = "tls"))]
									("tls", _) => {
										log(LogLevel::Error, "✗ TLS support is disabled in this build.");
									}
									("http", ConnectionObject::Tcp(stream)) => {
										tokio::spawn(async move {
											if let Err(e) = plain::run(stream, &mut kv_store, parent_path, "http").await {
												log(LogLevel::Error, &format!("✗ HTTP Carrier failed: {e:#}"));
											}
										});
									}
									// FIXED: Create an owned String for the closure to capture
									(proto_str, ConnectionObject::Tcp(stream)) => {
										let proto_owned = proto_str.to_owned();
										tokio::spawn(async move {
											if let Err(e) =
												plain::run(stream, &mut kv_store, parent_path, &proto_owned).await
											{
												log(
													LogLevel::Error,
													&format!("✗ Plain Carrier ({proto_owned}) failed: {e:#}"),
												);
											}
										});
									}
									(p, _) => {
										log(
											LogLevel::Error,
											&format!("✗ Unsupported upgrade protocol '{p}' or object mismatch."),
										);
									}
								}
							}
							Err(e) => {
								log(LogLevel::Error, &format!("✗ Flow execution failed: {e}"));
							}
						}
					} else {
						log(LogLevel::Debug, "⚙ Connection closed before data.");
					}
				}
				Err(e) => {
					log(LogLevel::Warn, &format!("⚠ Failed to peek: {e}"));
				}
			}
		}
	}
}
