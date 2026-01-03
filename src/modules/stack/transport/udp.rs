/* src/modules/stack/transport/udp.rs */

use super::{context, flow, legacy};
use crate::modules::{
	kv::KvStore,
	plugins::core::model::{ConnectionObject, Layer, ProcessingStep, TerminatorResult},
	stack::carrier,
};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use validator::{Validate, ValidationErrors};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FlowConfig {
	pub connection: ProcessingStep,
}

impl Validate for FlowConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		super::validator::validate_flow_config(&self.connection, Layer::L4, "udp")
	}
}

// --- Unified Configuration Enum ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum UdpConfig {
	Flow(FlowConfig),
	Legacy(legacy::LegacyUdpConfig),
}

impl Validate for UdpConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			UdpConfig::Legacy(config) => {
				let mut result = config.validate();
				if let Err(e) = legacy::validate_udp_rules(&config.rules) {
					match result {
						Ok(()) => {
							let mut errors = ValidationErrors::new();
							errors.add("rules", e);
							result = Err(errors);
						}
						Err(ref mut errors) => {
							errors.add("rules", e);
						}
					}
				}
				result
			}
			UdpConfig::Flow(config) => config.validate(),
		}
	}
}

// --- Main Dispatcher ---

pub async fn dispatch_udp_datagram(
	socket: Arc<UdpSocket>,
	port: u16,
	config: Arc<UdpConfig>,
	datagram: Vec<u8>,
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
				&format!("⚙ Entering Flow Engine path for UDP from {}.", client_addr),
			);

			context::populate_udp_context(&datagram, &mut kv_store);

			let conn_object = ConnectionObject::Udp {
				socket: socket.clone(),
				datagram: datagram.to_vec(),
				client_addr,
			};
			let result = flow::execute(
				&flow_config.connection,
				&mut kv_store,
				conn_object,
				std::collections::HashMap::new(),
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
						&format!("➜ Upgrading UDP flow to: {}", protocol),
					);

					match (protocol.as_str(), conn) {
						#[cfg(feature = "quic")]
						("quic", conn_obj) => {
							tokio::spawn(async move {
								if let Err(e) = carrier::quic::quic::run(conn_obj, &mut kv_store, parent_path).await
								{
									log(LogLevel::Error, &format!("✗ QUIC Carrier failed: {:#}", e));
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
								&format!("✗ Unsupported upgrade protocol '{}' for UDP flow.", p),
							);
						}
					}
				}
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ UDP Flow execution failed for {}: {:#}", client_addr, e),
					);
				}
			}
		}
	}
}
