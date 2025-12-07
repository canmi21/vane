/* src/modules/kv/mod.rs */

pub mod plugin_output;

use chrono::Utc;
use std::{collections::HashMap, net::SocketAddr};
use uuid::Uuid;

/// A per-connection, key-value storage space.
///
/// Keys are expected to be lowercase and dot-separated (e.g., "conn.ip").
/// All values are stored as strings.
pub type KvStore = HashMap<String, String>;

/// Creates a new, pre-populated KvStore for an incoming connection.
///
/// This function initializes the store with essential connection metadata,
/// including a unique UUID, source address, and timestamp.
///
/// # Arguments
///
/// * `peer_addr` - The remote socket address of the incoming connection.
/// * `protocol` - The protocol identifier ("tcp" or "udp").
///
/// # Returns
///
/// A `KvStore` instance populated with initial key-value pairs.
pub fn new(peer_addr: &SocketAddr, protocol: &str) -> KvStore {
	let mut kv = KvStore::new();

	// CORRECTED: Use `now_v7` to automatically generate a UUID from the current time.
	let uuid = Uuid::now_v7().to_string().replace('-', "");

	kv.insert("conn.uuid".to_string(), uuid);
	kv.insert("conn.ip".to_string(), peer_addr.ip().to_string());
	kv.insert("conn.port".to_string(), peer_addr.port().to_string());
	kv.insert("conn.proto".to_string(), protocol.to_lowercase());
	kv.insert(
		"conn.timestamp".to_string(),
		Utc::now().timestamp().to_string(),
	);

	kv
}
