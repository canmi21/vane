/* src/ingress/model.rs */

use crate::layers::l4::{tcp::TcpConfig, udp::UdpConfig};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::Serialize;
use std::sync::Arc;
use tokio::{sync::oneshot, time::Instant};

/// Represents the network protocol a port is listening on.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
	Tcp,
	Udp,
}

/// Represents the desired configuration state of a port.
#[derive(Serialize, Debug, Clone)]
pub struct PortStatus {
	pub port: u16,
	pub active: bool,
	#[serde(skip_serializing_if = "Option::is_none", with = "serde_arc")]
	pub tcp_config: Option<Arc<TcpConfig>>,
	#[serde(skip_serializing_if = "Option::is_none", with = "serde_arc")]
	pub udp_config: Option<Arc<UdpConfig>>,
}

/// A thread-safe, atomically swappable container for the list of port configurations.
pub type PortState = Arc<ArcSwap<Vec<PortStatus>>>;

// NEW: A global static reference to the PortState for easy access from tasks.
pub static CONFIG_STATE: Lazy<PortState> =
	Lazy::new(|| Arc::new(ArcSwap::new(Arc::new(Vec::new()))));

/// Represents the runtime state of an individual network listener task.
pub enum ListenerState {
	Active,
	Draining { since: Instant },
}

/// A handle to a running tokio task that is listening on a port.
pub struct RunningListener {
	pub state: Arc<tokio::sync::Mutex<ListenerState>>,
	pub shutdown_tx: oneshot::Sender<()>,
}

/// The global, thread-safe registry of all active and draining listener tasks.
pub static TASK_REGISTRY: Lazy<DashMap<(u16, Protocol), RunningListener>> = Lazy::new(DashMap::new);

/// Helper module for serializing Option<Arc<T>> in JSON responses.
mod serde_arc {
	use serde::{Serialize, Serializer};
	use std::sync::Arc;

	pub fn serialize<S, T>(val: &Option<Arc<T>>, s: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
		T: Serialize,
	{
		match val {
			Some(arc) => T::serialize(arc.as_ref(), s),
			None => s.serialize_none(),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::layers::l4::{
		legacy::{
			LegacyTcpConfig,
			tcp::{TcpDestination, TcpProtocolRule},
		},
		model::{Detect, DetectMethod, Forward, Strategy},
		tcp::TcpConfig,
	};
	use serde_json::json;

	/// Tests that the Protocol enum serializes to the correct lowercase string.
	#[test]
	fn test_protocol_serialization() {
		let tcp = Protocol::Tcp;
		let udp = Protocol::Udp;
		assert_eq!(serde_json::to_string(&tcp).unwrap(), "\"tcp\"");
		assert_eq!(serde_json::to_string(&udp).unwrap(), "\"udp\"");
	}

	/// Tests the serialization logic of the PortStatus struct.
	#[test]
	fn test_port_status_serialization() {
		// CORRECTED: Build the test case by creating the inner `LegacyTcpConfig` struct
		// first, and then wrapping it in the `TcpConfig::Legacy` enum variant.
		let dummy_legacy_config = LegacyTcpConfig {
			rules: vec![TcpProtocolRule {
				name: "test".to_string(),
				priority: 1,
				detect: Detect {
					method: DetectMethod::Fallback,
					pattern: "any".to_string(),
				},
				session: None,
				destination: TcpDestination::Forward {
					forward: Forward {
						strategy: Strategy::Random,
						targets: vec![],
						fallbacks: vec![],
					},
				},
			}],
		};
		let dummy_tcp_config = Arc::new(TcpConfig::Legacy(dummy_legacy_config));

		// 1. Test case with a TCP config present.
		let full_status = PortStatus {
			port: 8080,
			active: true,
			tcp_config: Some(dummy_tcp_config.clone()),
			udp_config: None,
		};

		let full_json = serde_json::to_value(&full_status).unwrap();
		let expected_full_json = json!({
			"port": 8080,
			"active": true,
			"tcp_config": {
				"protocols": [{
					"name": "test",
					"priority": 1,
					"detect": { "method": "fallback", "pattern": "any" },
					"session": null,
					"destination": {
						"type": "forward",
						"forward": {
							"strategy": "random",
							"targets": [],
							"fallbacks": []
						}
					}
				}]
			}
		});
		assert_eq!(full_json, expected_full_json);

		// 2. Test case with no configs, ensuring they are skipped.
		let minimal_status = PortStatus {
			port: 9090,
			active: false,
			tcp_config: None,
			udp_config: None,
		};

		let minimal_json = serde_json::to_value(&minimal_status).unwrap();
		let expected_minimal_json = json!({
			"port": 9090,
			"active": false
		});
		assert_eq!(minimal_json, expected_minimal_json);
	}
}
