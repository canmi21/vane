use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnLayer {
	L4,
	L5,
	L7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnPhase {
	Accepted,
	Detecting,
	Forwarding,
	TlsHandshake,
}

#[derive(Debug, Clone)]
pub struct ConnectionState {
	pub id: String,
	pub peer_addr: SocketAddr,
	pub server_addr: SocketAddr,
	pub layer: ConnLayer,
	pub phase: ConnPhase,
	pub protocol: Option<String>,
	pub tls_sni: Option<String>,
	pub tls_version: Option<String>,
	pub forward_target: Option<SocketAddr>,
	pub started_at: Instant,
}

/// Real-time registry of active connections.
///
/// Uses `DashMap` for shard-level locking -- updates are O(1) with no global lock.
/// Entries are automatically removed when the corresponding [`RegistryGuard`] is dropped.
pub struct ConnectionRegistry {
	inner: DashMap<String, ConnectionState>,
}

impl ConnectionRegistry {
	pub fn new() -> Self {
		Self { inner: DashMap::new() }
	}

	/// Register a connection and return an RAII guard that deregisters on drop.
	pub fn register(self: &Arc<Self>, state: ConnectionState) -> RegistryGuard {
		let id = state.id.clone();
		self.inner.insert(id.clone(), state);
		RegistryGuard { registry: self.clone(), id }
	}

	pub fn get(&self, id: &str) -> Option<ConnectionState> {
		self.inner.get(id).map(|r| r.value().clone())
	}

	pub fn count(&self) -> usize {
		self.inner.len()
	}

	pub fn snapshot(&self) -> Vec<ConnectionState> {
		self.inner.iter().map(|r| r.value().clone()).collect()
	}
}

impl Default for ConnectionRegistry {
	fn default() -> Self {
		Self::new()
	}
}

/// RAII guard that removes the connection entry on drop.
pub struct RegistryGuard {
	registry: Arc<ConnectionRegistry>,
	id: String,
}

impl RegistryGuard {
	pub fn update_phase(&self, phase: ConnPhase) {
		if let Some(mut entry) = self.registry.inner.get_mut(&self.id) {
			entry.phase = phase;
		}
	}

	pub fn update_layer(&self, layer: ConnLayer) {
		if let Some(mut entry) = self.registry.inner.get_mut(&self.id) {
			entry.layer = layer;
		}
	}

	pub fn set_protocol(&self, protocol: String) {
		if let Some(mut entry) = self.registry.inner.get_mut(&self.id) {
			entry.protocol = Some(protocol);
		}
	}

	pub fn set_tls_info(&self, sni: Option<String>, version: Option<String>) {
		if let Some(mut entry) = self.registry.inner.get_mut(&self.id) {
			entry.tls_sni = sni;
			entry.tls_version = version;
		}
	}

	pub fn set_forward_target(&self, addr: SocketAddr) {
		if let Some(mut entry) = self.registry.inner.get_mut(&self.id) {
			entry.forward_target = Some(addr);
		}
	}

	pub fn id(&self) -> &str {
		&self.id
	}
}

impl Drop for RegistryGuard {
	fn drop(&mut self) {
		self.registry.inner.remove(&self.id);
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;
	use std::net::{IpAddr, Ipv4Addr};

	fn test_state(id: &str) -> ConnectionState {
		ConnectionState {
			id: id.to_owned(),
			peer_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 12345),
			server_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
			layer: ConnLayer::L4,
			phase: ConnPhase::Accepted,
			protocol: None,
			tls_sni: None,
			tls_version: None,
			forward_target: None,
			started_at: Instant::now(),
		}
	}

	#[test]
	fn register_and_get() {
		let registry = Arc::new(ConnectionRegistry::new());
		let _guard = registry.register(test_state("conn-1"));

		let retrieved = registry.get("conn-1").unwrap();
		assert_eq!(retrieved.id, "conn-1");
		assert_eq!(retrieved.layer, ConnLayer::L4);
		assert_eq!(retrieved.phase, ConnPhase::Accepted);
	}

	#[test]
	fn count_and_snapshot() {
		let registry = Arc::new(ConnectionRegistry::new());
		let _g1 = registry.register(test_state("a"));
		let _g2 = registry.register(test_state("b"));

		assert_eq!(registry.count(), 2);
		let snap = registry.snapshot();
		assert_eq!(snap.len(), 2);
	}

	#[test]
	fn guard_drop_deregisters() {
		let registry = Arc::new(ConnectionRegistry::new());
		let guard = registry.register(test_state("x"));
		assert_eq!(registry.count(), 1);

		drop(guard);
		assert_eq!(registry.count(), 0);
		assert!(registry.get("x").is_none());
	}

	#[test]
	fn guard_update_methods() {
		let registry = Arc::new(ConnectionRegistry::new());
		let guard = registry.register(test_state("u"));

		guard.update_phase(ConnPhase::Detecting);
		assert_eq!(registry.get("u").unwrap().phase, ConnPhase::Detecting);

		guard.update_layer(ConnLayer::L5);
		assert_eq!(registry.get("u").unwrap().layer, ConnLayer::L5);

		guard.set_protocol("tls".to_owned());
		assert_eq!(registry.get("u").unwrap().protocol.as_deref(), Some("tls"));

		guard.set_tls_info(Some("example.com".to_owned()), Some("TLSv1.3".to_owned()));
		let state = registry.get("u").unwrap();
		assert_eq!(state.tls_sni.as_deref(), Some("example.com"));
		assert_eq!(state.tls_version.as_deref(), Some("TLSv1.3"));

		let target: SocketAddr = "10.0.0.1:3000".parse().unwrap();
		guard.set_forward_target(target);
		assert_eq!(registry.get("u").unwrap().forward_target, Some(target));
	}

	#[tokio::test]
	async fn concurrent_register_deregister() {
		let registry = Arc::new(ConnectionRegistry::new());
		let mut handles = Vec::new();

		for i in 0..100 {
			let reg = registry.clone();
			handles.push(tokio::spawn(async move {
				let guard = reg.register(test_state(&format!("conn-{i}")));
				tokio::task::yield_now().await;
				drop(guard);
			}));
		}

		for handle in handles {
			handle.await.unwrap();
		}

		assert_eq!(registry.count(), 0);
	}

	#[test]
	fn set_tls_info_clears_with_none() {
		let registry = Arc::new(ConnectionRegistry::new());
		let guard = registry.register(test_state("tls-clear"));

		// Set TLS info
		guard.set_tls_info(Some("example.com".to_owned()), Some("TLSv1.3".to_owned()));
		let state = registry.get("tls-clear").unwrap();
		assert_eq!(state.tls_sni.as_deref(), Some("example.com"));
		assert_eq!(state.tls_version.as_deref(), Some("TLSv1.3"));

		// Clear TLS info
		guard.set_tls_info(None, None);
		let state = registry.get("tls-clear").unwrap();
		assert!(state.tls_sni.is_none());
		assert!(state.tls_version.is_none());
	}

	#[tokio::test]
	async fn concurrent_updates_same_connection() {
		let registry = Arc::new(ConnectionRegistry::new());
		let guard = Arc::new(registry.register(test_state("concurrent")));
		let mut handles = Vec::new();

		for i in 0..50 {
			let g = guard.clone();
			handles.push(tokio::spawn(async move {
				if i % 2 == 0 {
					g.update_phase(ConnPhase::Forwarding);
				} else {
					g.update_layer(ConnLayer::L5);
				}
			}));
		}

		for handle in handles {
			handle.await.unwrap();
		}

		// Connection entry should still exist and be consistent (no panic, no corruption)
		let state = registry.get("concurrent").unwrap();
		assert_eq!(state.id, "concurrent");
	}
}
