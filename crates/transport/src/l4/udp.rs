// Config types now live in vane-engine
pub use vane_engine::config::{UdpConfig, UdpFlowConfig as FlowConfig};

// Dispatch function stays here (will move to vane-transport in Step 5)
use super::{context, flow, legacy};
use vane_engine::engine::interfaces::{ConnectionObject, TerminatorResult};

use crate::l4p::quic;
use fancy_log::{LogLevel, log};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use vane_primitives::kv::KvStore;

pub async fn dispatch_udp_datagram(
	socket: Arc<UdpSocket>,
	port: u16,
	config: Arc<UdpConfig>,
	datagram: bytes::Bytes,
	client_addr: SocketAddr,
	mut kv_store: KvStore,
) {
	match &*config {
		// 1. LEGACY MODE (Strict Backward Compatibility)
		UdpConfig::Legacy(legacy_config) => {
			legacy::dispatch_legacy_udp(socket, port, legacy_config, &datagram, client_addr).await;
		}

		// 2. FLOW MODE
		UdpConfig::Flow(flow_config) => {
			log(
				LogLevel::Debug,
				&format!("⚙ Entering Flow Engine path for UDP from {client_addr}."),
			);

			context::populate_udp_context(&datagram, &mut kv_store);

			let conn_object = ConnectionObject::Udp {
				socket: socket.clone(),
				datagram: datagram.clone(),
				client_addr,
			};
			let result = flow::execute(
				&flow_config.connection,
				&mut kv_store,
				conn_object,
				ahash::AHashMap::new(),
			)
			.await;
			match result {
				Ok(TerminatorResult::Finished) => {
					log(LogLevel::Debug, "✓ UDP Flow handled at L4.");
				}
				Ok(TerminatorResult::Upgrade {
					protocol,
					conn,
					parent_path,
				}) => {
					log(
						LogLevel::Info,
						&format!("➜ Upgrading UDP flow to: {protocol}"),
					);

					match (protocol.as_str(), conn) {
						#[cfg(feature = "quic")]
						("quic", conn_obj) => {
							tokio::spawn(async move {
								if let Err(e) = quic::protocol::run(conn_obj, &mut kv_store, parent_path).await {
									log(LogLevel::Error, &format!("✗ QUIC Carrier failed: {e:#}"));
								}
							});
						}
						#[cfg(not(feature = "quic"))]
						("quic", _) => {
							log(LogLevel::Error, "✗ QUIC support is disabled in this build.");
						}
						(p, _) => {
							log(
								LogLevel::Error,
								&format!("✗ Unsupported upgrade protocol '{p}' for UDP flow."),
							);
						}
					}
				}
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ UDP Flow execution failed for {client_addr}: {e:#}"),
					);
				}
			}
		}
	}
}
