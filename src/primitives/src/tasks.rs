/* src/ingress/tasks.rs */

use dashmap::DashMap;

use once_cell::sync::Lazy;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// --- Connection Tracking ---

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

#[derive(Clone, Debug)]
pub struct ConnectionGuard(#[allow(dead_code)] Arc<InternalGuard>);

#[derive(Debug)]
pub struct ConnectionTracker {
	global_count: AtomicUsize,
	ip_counts: DashMap<IpAddr, AtomicUsize>,
	max_connections: usize,
	max_connections_per_ip: usize,
}

impl ConnectionTracker {
	fn new() -> Self {
		let max_conn = envflag::get::<usize>("MAX_CONNECTIONS", 10000);
		let max_per_ip = envflag::get::<usize>("MAX_CONNECTIONS_PER_IP", 50);

		Self {
			global_count: AtomicUsize::new(0),
			ip_counts: DashMap::new(),
			max_connections: max_conn,
			max_connections_per_ip: max_per_ip,
		}
	}

	pub fn acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnectionGuard> {
		// 1. Check global limit
		let current_global = self.global_count.load(Ordering::Relaxed);
		if current_global >= self.max_connections {
			return None;
		}

		// 2. Check IP limit
		let ip_entry = self.ip_counts.entry(ip).or_insert_with(|| AtomicUsize::new(0));
		let current_ip = ip_entry.load(Ordering::Relaxed);
		if current_ip >= self.max_connections_per_ip {
			return None;
		}

		// 3. Increment counters
		self.global_count.fetch_add(1, Ordering::Relaxed);
		ip_entry.fetch_add(1, Ordering::Relaxed);

		Some(ConnectionGuard(Arc::new(InternalGuard { tracker: self.clone(), ip })))
	}

	fn release(&self, ip: IpAddr) {
		self.global_count.fetch_sub(1, Ordering::Relaxed);
		if let Some(ip_count) = self.ip_counts.get(&ip) {
			let prev = ip_count.fetch_sub(1, Ordering::Relaxed);
			if prev == 1 {
				// Count dropped to 0, clean up the entry to save memory
				drop(ip_count);
				self.ip_counts.remove_if(&ip, |_, count| count.load(Ordering::Relaxed) == 0);
			}
		}
	}
}

pub static GLOBAL_TRACKER: Lazy<Arc<ConnectionTracker>> =
	Lazy::new(|| Arc::new(ConnectionTracker::new()));
