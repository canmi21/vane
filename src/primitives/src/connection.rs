use dashmap::DashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Tracks active connections with global and per-IP limits.
#[derive(Debug)]
pub struct ConnectionTracker {
	global_count: AtomicUsize,
	ip_counts: DashMap<IpAddr, AtomicUsize>,
	max_connections: usize,
	max_connections_per_ip: usize,
}

impl ConnectionTracker {
	#[must_use]
	pub fn new(max_connections: usize, max_connections_per_ip: usize) -> Self {
		Self {
			global_count: AtomicUsize::new(0),
			ip_counts: DashMap::new(),
			max_connections,
			max_connections_per_ip,
		}
	}

	/// Try to acquire a connection slot. Returns a guard that releases on drop.
	pub fn acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnectionGuard> {
		let current_global = self.global_count.load(Ordering::Relaxed);
		if current_global >= self.max_connections {
			return None;
		}

		let ip_entry = self.ip_counts.entry(ip).or_insert_with(|| AtomicUsize::new(0));
		let current_ip = ip_entry.load(Ordering::Relaxed);
		if current_ip >= self.max_connections_per_ip {
			return None;
		}

		self.global_count.fetch_add(1, Ordering::Relaxed);
		ip_entry.fetch_add(1, Ordering::Relaxed);
		drop(ip_entry);

		Some(ConnectionGuard(Arc::new(InternalGuard { tracker: self.clone(), ip })))
	}

	fn release(&self, ip: IpAddr) {
		self.global_count.fetch_sub(1, Ordering::Relaxed);
		if let Some(ip_count) = self.ip_counts.get(&ip) {
			let prev = ip_count.fetch_sub(1, Ordering::Relaxed);
			if prev == 1 {
				drop(ip_count);
				self.ip_counts.remove_if(&ip, |_, count| count.load(Ordering::Relaxed) == 0);
			}
		}
	}

	#[must_use]
	pub fn global_count(&self) -> usize {
		self.global_count.load(Ordering::Relaxed)
	}

	#[must_use]
	pub fn ip_count(&self, ip: &IpAddr) -> usize {
		self.ip_counts.get(ip).map_or(0, |c| c.load(Ordering::Relaxed))
	}
}

/// RAII guard that releases a connection slot when all clones are dropped.
#[derive(Clone, Debug)]
pub struct ConnectionGuard(#[allow(dead_code)] Arc<InternalGuard>);

#[derive(Debug)]
struct InternalGuard {
	tracker: Arc<ConnectionTracker>,
	ip: IpAddr,
}

impl Drop for InternalGuard {
	fn drop(&mut self) {
		self.tracker.release(self.ip);
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;
	use std::net::Ipv4Addr;

	fn localhost() -> IpAddr {
		IpAddr::V4(Ipv4Addr::LOCALHOST)
	}

	fn other_ip() -> IpAddr {
		IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
	}

	#[test]
	fn acquire_succeeds() {
		let tracker = Arc::new(ConnectionTracker::new(10, 5));
		let guard = tracker.acquire(localhost());
		assert!(guard.is_some());
		assert_eq!(tracker.global_count(), 1);
		assert_eq!(tracker.ip_count(&localhost()), 1);
	}

	#[test]
	fn global_limit_rejects() {
		let tracker = Arc::new(ConnectionTracker::new(2, 10));
		let _g1 = tracker.acquire(localhost()).unwrap();
		let _g2 = tracker.acquire(other_ip()).unwrap();
		assert!(tracker.acquire(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))).is_none());
	}

	#[test]
	fn per_ip_limit_rejects_same_ip_allows_different() {
		let tracker = Arc::new(ConnectionTracker::new(100, 1));
		let _g1 = tracker.acquire(localhost()).unwrap();
		// Same IP should be rejected
		assert!(tracker.acquire(localhost()).is_none());
		// Different IP should succeed
		assert!(tracker.acquire(other_ip()).is_some());
	}

	#[test]
	fn guard_drop_releases_count() {
		let tracker = Arc::new(ConnectionTracker::new(10, 5));
		let guard = tracker.acquire(localhost()).unwrap();
		assert_eq!(tracker.global_count(), 1);
		drop(guard);
		assert_eq!(tracker.global_count(), 0);
		assert_eq!(tracker.ip_count(&localhost()), 0);
	}

	#[test]
	fn guard_clone_shares_ownership() {
		let tracker = Arc::new(ConnectionTracker::new(10, 5));
		let guard = tracker.acquire(localhost()).unwrap();
		let clone = guard.clone();

		// Both exist — count stays at 1
		assert_eq!(tracker.global_count(), 1);

		drop(guard);
		// Clone still alive — count stays at 1
		assert_eq!(tracker.global_count(), 1);

		drop(clone);
		// All dropped — count back to 0
		assert_eq!(tracker.global_count(), 0);
	}

	#[test]
	fn ip_entry_cleaned_after_release() {
		let tracker = Arc::new(ConnectionTracker::new(10, 5));
		let guard = tracker.acquire(localhost()).unwrap();
		assert_eq!(tracker.ip_count(&localhost()), 1);
		drop(guard);
		assert_eq!(tracker.ip_count(&localhost()), 0);
	}
}
