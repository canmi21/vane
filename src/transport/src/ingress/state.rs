/* src/transport/src/ingress/state.rs */

use dashmap::DashMap;
use serde::Serialize;
use sigterm::ShutdownHandle;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::time::Instant;

/// Represents the network protocol a port is listening on.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
	Tcp,
	Udp,
}

/// Represents the runtime state of an individual network listener task.
pub enum ListenerState {
	Active,
	Draining { since: Instant },
}

/// A handle to a running tokio task that is listening on a port.
pub struct RunningListener {
	pub state: Arc<tokio::sync::Mutex<ListenerState>>,
	pub shutdown: ShutdownHandle,
}

/// The global, thread-safe registry of all active and draining listener tasks.
pub static TASK_REGISTRY: LazyLock<DashMap<(u16, Protocol), RunningListener>> =
	LazyLock::new(DashMap::new);

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json;

	/// Tests that the Protocol enum serializes to the correct lowercase string.
	#[test]
	fn test_protocol_serialization() {
		let tcp = Protocol::Tcp;
		let udp = Protocol::Udp;
		assert_eq!(serde_json::to_string(&tcp).unwrap(), "\"tcp\"");
		assert_eq!(serde_json::to_string(&udp).unwrap(), "\"udp\"");
	}
}
