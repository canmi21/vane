/* engine/src/modules/ratelimit/heap.rs */

use std::time::{Duration, Instant};

const ONE_SECOND: Duration = Duration::from_secs(1);

/// Represents a collection of request timestamps for a single rate-limiting key.
#[derive(Debug)]
pub struct RequestHeap {
	timestamps: Vec<Instant>,
}

impl RequestHeap {
	/// Creates a new, empty heap.
	pub fn new() -> Self {
		Self {
			timestamps: Vec::new(),
		}
	}

	/// Adds a new request timestamp and checks if the rate limit has been exceeded.
	/// It automatically prunes timestamps older than one second.
	pub fn add_and_check(&mut self, now: Instant, limit: u32) -> bool {
		// Prune timestamps older than 1 second from the provided 'now'.
		self
			.timestamps
			.retain(|&t| now.duration_since(t) < ONE_SECOND);

		// Check if the current request count is under the limit.
		if (self.timestamps.len() as u32) < limit {
			self.timestamps.push(now);
			true // Request is allowed
		} else {
			false // Request is denied
		}
	}

	/// Provides an estimation of the memory used by this heap in bytes.
	pub fn memory_size(&self) -> usize {
		self.timestamps.capacity() * std::mem::size_of::<Instant>()
	}

	/// Forcibly removes the oldest `count` entries to reclaim memory.
	/// Used by the Garbage Collector.
	pub fn prune_oldest(&mut self, count: usize) {
		if count >= self.timestamps.len() {
			self.timestamps.clear();
		} else {
			// Drains the first `count` elements.
			self.timestamps.drain(0..count);
		}
	}
}
