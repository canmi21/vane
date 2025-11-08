/* src/modules/ports/model.rs */

use super::super::server::l4::model::{TcpConfig, UdpConfig};
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
