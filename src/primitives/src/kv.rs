/* src/primitives/src/kv.rs */

use ahash::AHashMap;
use chrono::Utc;
use std::net::SocketAddr;
use uuid::Uuid;

/// A per-connection, key-value storage space.
///
/// Keys are expected to be lowercase and dot-separated (e.g., "conn.ip").
/// All values are stored as strings.
/// Using AHashMap for high-performance variable resolution.
pub type KvStore = AHashMap<String, String>;

/// Creates a new, pre-populated KvStore for an incoming connection.
///
/// This function initializes the store with essential connection metadata,
/// including a unique UUID, source address, server address, and timestamp.
///
/// # Arguments
///
/// * `peer_addr` - The remote socket address of the incoming connection.
/// * `server_addr` - The local socket address of the listening server.
/// * `protocol` - The protocol identifier ("tcp" or "udp").
///
/// # Returns
///
/// A `KvStore` instance populated with initial key-value pairs.
#[must_use]
pub fn new(peer_addr: &SocketAddr, server_addr: &SocketAddr, protocol: &str) -> KvStore {
	let mut kv = KvStore::new();

	// UUIDv7 as time-related connection id
	let uuid = Uuid::now_v7().to_string().replace('-', "");

	kv.insert("conn.uuid".to_owned(), uuid);
	kv.insert("conn.ip".to_owned(), peer_addr.ip().to_string());
	kv.insert("conn.port".to_owned(), peer_addr.port().to_string());
	kv.insert("conn.proto".to_owned(), protocol.to_lowercase());
	kv.insert("conn.timestamp".to_owned(), Utc::now().timestamp().to_string());
	kv.insert("server.ip".to_owned(), server_addr.ip().to_string());
	kv.insert("server.port".to_owned(), server_addr.port().to_string());

	kv
}
