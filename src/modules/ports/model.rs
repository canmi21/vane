/* src/modules/ports/model.rs */

use arc_swap::ArcSwap;
use serde::Serialize;
use std::sync::Arc;

/// Represents the protocol a port is listening on.
// Add Hash and Eq to allow using this in a HashSet for easy comparison.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
	Tcp,
	Udp,
}

/// Represents the live, in-memory status of a single managed port.
#[derive(Serialize, Debug, Clone)]
pub struct PortStatus {
	pub port: u16,
	pub active: bool,
	pub protocols: Vec<Protocol>,
}

/// A thread-safe, atomically swappable container for the entire list of port statuses.
pub type PortState = Arc<ArcSwap<Vec<PortStatus>>>;
