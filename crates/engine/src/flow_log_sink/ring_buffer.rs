use std::collections::VecDeque;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use vane_core::{FlowLogEvent, FlowLogSink};

/// In-memory tail-log ring buffer. TTL is keyed on the **server-side**
/// `Instant` at emit, not the producer-supplied `event.t` field —
/// `event.t` is operator-controlled (executor / middleware / WASM
/// guest can in principle stuff any wall-clock value in there) and
/// using it for eviction would let a producer with a skewed or
/// adversarial clock either pin entries forever (`t = u64::MAX`) or
/// evict every sibling on the next push (`t = 0`).
pub struct RingBufferSink {
	inner: Mutex<VecDeque<RingEntry>>,
	cap: usize,
	ttl: Duration,
}

struct RingEntry {
	emitted_at: Instant,
	event: FlowLogEvent,
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
	/// management API's `tail_flow` backfill.
	#[must_use]
	pub fn snapshot(&self) -> Vec<FlowLogEvent> {
		self.inner.lock().iter().map(|e| e.event.clone()).collect()
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
		// Capture the server-side timestamp ONCE per emit. This is the
		// only clock the eviction loop trusts — see the type doc on
		// why `event.t` is unsuitable.
		let now = Instant::now();
		let mut q = self.inner.lock();

		// Evict expired from the front based on server-side time.
		while let Some(front) = q.front() {
			if now.duration_since(front.emitted_at) > self.ttl {
				q.pop_front();
			} else {
				break;
			}
		}

		// Cap-evict oldest before pushing.
		if q.len() >= self.cap {
			q.pop_front();
		}
		q.push_back(RingEntry { emitted_at: now, event });
	}
}
