/* src/modules/ports/model.rs */

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

/// Represents the desired configuration state of a port, as read from the filesystem.
#[derive(Serialize, Debug, Clone)]
pub struct PortStatus {
	pub port: u16,
	pub active: bool,
	pub protocols: Vec<Protocol>,
}

/// A thread-safe, atomically swappable container for the list of port configurations.
/// This is exposed via the API.
pub type PortState = Arc<ArcSwap<Vec<PortStatus>>>;

/// Represents the runtime state of an individual network listener task.
pub enum ListenerState {
	Active,
	/// The listener is shutting down and will no longer accept new connections.
	Draining {
		since: Instant,
	},
}

/// A handle to a running tokio task that is listening on a port.
pub struct RunningListener {
	pub state: Arc<tokio::sync::Mutex<ListenerState>>,
	/// A channel used to send a shutdown signal to the listener task.
	pub shutdown_tx: oneshot::Sender<()>,
}

/// The global, thread-safe registry of all active and draining listener tasks.
pub static TASK_REGISTRY: Lazy<DashMap<(u16, Protocol), RunningListener>> = Lazy::new(DashMap::new);
