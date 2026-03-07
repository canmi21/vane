use ahash::AHashMap;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const KEY_CONN_UUID: &str = "conn.uuid";
pub const KEY_CONN_IP: &str = "conn.ip";
pub const KEY_CONN_PORT: &str = "conn.port";
pub const KEY_CONN_PROTO: &str = "conn.proto";
pub const KEY_CONN_TIMESTAMP: &str = "conn.timestamp";
pub const KEY_SERVER_IP: &str = "server.ip";
pub const KEY_SERVER_PORT: &str = "server.port";

/// Per-connection key-value store with pre-populated connection metadata.
///
/// ```
/// use std::net::{IpAddr, Ipv4Addr, SocketAddr};
/// use vane_primitives::kv::KvStore;
///
/// let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 54321);
/// let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080);
/// let mut kv = KvStore::new(&peer, &server, "TCP");
///
/// assert_eq!(kv.conn_ip(), "192.168.1.100");
/// assert_eq!(kv.conn_proto(), "tcp"); // protocol is lowercased
///
/// kv.set("route.target".to_owned(), "backend-a".to_owned());
/// assert_eq!(kv.get("route.target"), Some("backend-a"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct KvStore {
	inner: AHashMap<String, String>,
}

impl KvStore {
	/// Create a new store populated with connection metadata.
	///
	/// Generates a `UUIDv7` (32-char hex, no dashes) and normalizes protocol to lowercase.
	#[must_use]
	#[allow(clippy::expect_used)] // system clock before UNIX epoch is unrecoverable
	pub fn new(peer_addr: &SocketAddr, server_addr: &SocketAddr, protocol: &str) -> Self {
		let mut inner = AHashMap::with_capacity(8);

		let uuid = Uuid::now_v7().simple().to_string();

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.expect("system clock before UNIX epoch")
			.as_secs();

		inner.insert(KEY_CONN_UUID.to_owned(), uuid);
		inner.insert(KEY_CONN_IP.to_owned(), peer_addr.ip().to_string());
		inner.insert(KEY_CONN_PORT.to_owned(), peer_addr.port().to_string());
		inner.insert(KEY_CONN_PROTO.to_owned(), protocol.to_lowercase());
		inner.insert(KEY_CONN_TIMESTAMP.to_owned(), timestamp.to_string());
		inner.insert(KEY_SERVER_IP.to_owned(), server_addr.ip().to_string());
		inner.insert(KEY_SERVER_PORT.to_owned(), server_addr.port().to_string());

		Self { inner }
	}

	// Typed getters below are guaranteed present after new(); expect is a
	// structural invariant, not a runtime risk.
	#[allow(clippy::expect_used)]
	#[must_use]
	pub fn conn_uuid(&self) -> &str {
		self.inner.get(KEY_CONN_UUID).expect("conn.uuid missing")
	}

	#[allow(clippy::expect_used)]
	#[must_use]
	pub fn conn_ip(&self) -> &str {
		self.inner.get(KEY_CONN_IP).expect("conn.ip missing")
	}

	#[allow(clippy::expect_used)]
	#[must_use]
	pub fn conn_port(&self) -> &str {
		self.inner.get(KEY_CONN_PORT).expect("conn.port missing")
	}

	#[allow(clippy::expect_used)]
	#[must_use]
	pub fn conn_proto(&self) -> &str {
		self.inner.get(KEY_CONN_PROTO).expect("conn.proto missing")
	}

	#[allow(clippy::expect_used)]
	#[must_use]
	pub fn conn_timestamp(&self) -> &str {
		self.inner.get(KEY_CONN_TIMESTAMP).expect("conn.timestamp missing")
	}

	#[allow(clippy::expect_used)]
	#[must_use]
	pub fn server_ip(&self) -> &str {
		self.inner.get(KEY_SERVER_IP).expect("server.ip missing")
	}

	#[allow(clippy::expect_used)]
	#[must_use]
	pub fn server_port(&self) -> &str {
		self.inner.get(KEY_SERVER_PORT).expect("server.port missing")
	}

	// -- generic accessors --

	#[must_use]
	pub fn get(&self, key: &str) -> Option<&str> {
		self.inner.get(key).map(String::as_str)
	}

	pub fn set(&mut self, key: String, value: String) -> Option<String> {
		self.inner.insert(key, value)
	}

	pub fn remove(&mut self, key: &str) -> Option<String> {
		self.inner.remove(key)
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;
	use std::net::{IpAddr, Ipv4Addr};

	fn test_addrs() -> (SocketAddr, SocketAddr) {
		let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 54321);
		let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080);
		(peer, server)
	}

	#[test]
	fn new_populates_all_core_fields() {
		let (peer, server) = test_addrs();
		let kv = KvStore::new(&peer, &server, "TCP");

		assert!(kv.get(KEY_CONN_UUID).is_some());
		assert!(kv.get(KEY_CONN_IP).is_some());
		assert!(kv.get(KEY_CONN_PORT).is_some());
		assert!(kv.get(KEY_CONN_PROTO).is_some());
		assert!(kv.get(KEY_CONN_TIMESTAMP).is_some());
		assert!(kv.get(KEY_SERVER_IP).is_some());
		assert!(kv.get(KEY_SERVER_PORT).is_some());
	}

	#[test]
	fn typed_getters_return_correct_values() {
		let (peer, server) = test_addrs();
		let kv = KvStore::new(&peer, &server, "tcp");

		assert_eq!(kv.conn_ip(), "192.168.1.100");
		assert_eq!(kv.conn_port(), "54321");
		assert_eq!(kv.conn_proto(), "tcp");
		assert_eq!(kv.server_ip(), "0.0.0.0");
		assert_eq!(kv.server_port(), "8080");
	}

	#[test]
	fn uuid_is_32_hex_chars() {
		let (peer, server) = test_addrs();
		let kv = KvStore::new(&peer, &server, "tcp");
		let uuid = kv.conn_uuid();
		assert_eq!(uuid.len(), 32);
		assert!(uuid.chars().all(|c| c.is_ascii_hexdigit()));
	}

	#[test]
	fn protocol_lowercased() {
		let (peer, server) = test_addrs();
		let kv = KvStore::new(&peer, &server, "TCP");
		assert_eq!(kv.conn_proto(), "tcp");

		let kv2 = KvStore::new(&peer, &server, "Udp");
		assert_eq!(kv2.conn_proto(), "udp");
	}

	#[test]
	fn get_set_remove_roundtrip() {
		let (peer, server) = test_addrs();
		let mut kv = KvStore::new(&peer, &server, "tcp");

		assert!(kv.get("custom.key").is_none());
		kv.set("custom.key".to_owned(), "hello".to_owned());
		assert_eq!(kv.get("custom.key"), Some("hello"));

		let old = kv.set("custom.key".to_owned(), "world".to_owned());
		assert_eq!(old.as_deref(), Some("hello"));
		assert_eq!(kv.get("custom.key"), Some("world"));

		let removed = kv.remove("custom.key");
		assert_eq!(removed.as_deref(), Some("world"));
		assert!(kv.get("custom.key").is_none());
	}

	#[test]
	fn ipv6_peer_address() {
		let peer = SocketAddr::new(IpAddr::V6("::1".parse().unwrap()), 9999);
		let server = SocketAddr::new(IpAddr::V6("::1".parse().unwrap()), 443);
		let kv = KvStore::new(&peer, &server, "tcp");
		assert_eq!(kv.conn_ip(), "::1");
		assert_eq!(kv.server_ip(), "::1");
	}

	#[test]
	fn default_store_is_empty() {
		let kv = KvStore::default();
		assert!(kv.get(KEY_CONN_UUID).is_none());
		assert!(kv.get(KEY_CONN_IP).is_none());
		assert!(kv.get("anything").is_none());
	}
}
