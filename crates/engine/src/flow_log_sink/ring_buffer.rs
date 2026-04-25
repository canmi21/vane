use std::collections::VecDeque;
use std::time::Duration;

use parking_lot::Mutex;
use vane_core::{FlowLogEvent, FlowLogSink};

pub struct RingBufferSink {
	inner: Mutex<VecDeque<FlowLogEvent>>,
	cap: usize,
	ttl: Duration,
}

impl RingBufferSink {
	pub const DEFAULT_CAP: usize = 10_000;
	pub const DEFAULT_TTL: Duration = Duration::from_mins(1);

	#[must_use]
	pub fn new(cap: usize, ttl: Duration) -> Self {
		Self { inner: Mutex::new(VecDeque::with_capacity(cap.min(Self::DEFAULT_CAP))), cap, ttl }
	}

	#[must_use]
	pub fn with_defaults() -> Self {
		Self::new(Self::DEFAULT_CAP, Self::DEFAULT_TTL)
	}

	/// Snapshot the current ring contents in arrival order. Used by the
	/// management API's `tail_flow_log` backfill (S1-29).
	#[must_use]
	pub fn snapshot(&self) -> Vec<FlowLogEvent> {
		self.inner.lock().iter().cloned().collect()
	}

	#[must_use]
	pub fn len(&self) -> usize {
		self.inner.lock().len()
	}

	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.inner.lock().is_empty()
	}
}

impl FlowLogSink for RingBufferSink {
	fn emit(&self, event: FlowLogEvent) {
		let mut q = self.inner.lock();
		let now = event.t;
		let ttl_ms = u64::try_from(self.ttl.as_millis()).unwrap_or(u64::MAX);

		// Evict expired from the front.
		while let Some(front) = q.front() {
			if now.saturating_sub(front.t) > ttl_ms {
				q.pop_front();
			} else {
				break;
			}
		}

		// Cap-evict oldest before pushing.
		if q.len() >= self.cap {
			q.pop_front();
		}
		q.push_back(event);
	}
}
