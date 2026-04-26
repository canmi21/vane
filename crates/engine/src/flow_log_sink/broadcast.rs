//! `BroadcastSink` — fans `FlowLogEvent`s into a tokio broadcast
//! channel for streaming mgmt verbs (`tail_flow_log`).
//!
//! Bounded ring; lagging subscribers drop frames rather than back-
//! pressuring the executor. The executor calls `emit` once per event
//! regardless of how many subscribers are connected; broadcast's
//! per-subscriber backlog handling lives in the channel itself.

use tokio::sync::broadcast;
use vane_core::{FlowLogEvent, FlowLogSink};

/// Broadcast channel capacity. A subscriber that falls more than
/// `BROADCAST_CAP` events behind sees [`broadcast::error::RecvError::Lagged`]
/// and resumes from the next available event. The mgmt handler maps
/// that to a synthetic `kind:"lagged"` sentinel so the operator can
/// see they're getting a sampled view rather than the full stream.
const BROADCAST_CAP: usize = 1024;

pub struct BroadcastSink {
	tx: broadcast::Sender<FlowLogEvent>,
}

impl BroadcastSink {
	#[must_use]
	pub fn new() -> Self {
		// `broadcast::channel` returns `(Sender, Receiver)`; we discard the
		// initial receiver. Subscribers are created on demand via
		// `subscribe()`.
		let (tx, _initial_rx) = broadcast::channel(BROADCAST_CAP);
		Self { tx }
	}

	/// Subscribe to live events. Each subscriber gets its own receiver
	/// with independent backlog tracking.
	#[must_use]
	pub fn subscribe(&self) -> broadcast::Receiver<FlowLogEvent> {
		self.tx.subscribe()
	}

	/// Number of currently active subscribers. Useful for tests and for
	/// future mgmt instrumentation. Cheap — broadcast tracks this
	/// internally.
	#[must_use]
	pub fn subscriber_count(&self) -> usize {
		self.tx.receiver_count()
	}
}

impl FlowLogSink for BroadcastSink {
	fn emit(&self, event: FlowLogEvent) {
		// `send` returns `Err` only when there are no receivers. That's
		// the steady state when no mgmt client is tailing — drop quietly.
		let _ = self.tx.send(event);
	}
}

impl Default for BroadcastSink {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Arc;
	use vane_core::{ConnId, FlowLogKind};

	fn evt(seq: u32) -> FlowLogEvent {
		FlowLogEvent {
			t: u64::from(seq),
			conn: ConnId(1),
			seq,
			kind: FlowLogKind::Trajectory,
			node: None,
			error: None,
			data: None,
		}
	}

	#[tokio::test]
	async fn broadcast_sink_emits_to_subscriber() {
		let sink = BroadcastSink::new();
		let mut rx = sink.subscribe();
		assert_eq!(sink.subscriber_count(), 1);

		// Cast to `dyn FlowLogSink` to exercise the sink trait path the
		// daemon actually uses (FanoutSink fanout via Arc<dyn ...>).
		let dyn_sink: Arc<dyn FlowLogSink> = Arc::new(sink);
		dyn_sink.emit(evt(7));
		let got = rx.recv().await.expect("recv");
		assert_eq!(got.seq, 7);
	}

	#[tokio::test]
	async fn broadcast_sink_no_receivers_silently_drops() {
		// With zero receivers, broadcast::send returns Err. The sink
		// must not propagate that — emit's contract is "fire and forget".
		let sink = BroadcastSink::new();
		assert_eq!(sink.subscriber_count(), 0);
		// No panic, no unwrap, no return value.
		<BroadcastSink as FlowLogSink>::emit(&sink, evt(1));
	}

	#[tokio::test]
	async fn broadcast_sink_lagged_subscriber_sees_lagged_error() {
		// Saturate the channel beyond a single subscriber's buffer.
		// Sending more than BROADCAST_CAP without recv causes the
		// subscriber to see `Lagged(n)` on its next recv.
		let sink = BroadcastSink::new();
		let mut rx = sink.subscribe();
		let total = u32::try_from(BROADCAST_CAP).expect("BROADCAST_CAP fits in u32") + 5;
		for s in 0..total {
			<BroadcastSink as FlowLogSink>::emit(&sink, evt(s));
		}
		// The first recv should report a lag of at least 5 (we sent
		// CAP + 5 without the subscriber ever waking up).
		match rx.recv().await {
			Err(broadcast::error::RecvError::Lagged(n)) => {
				assert!(n >= 5, "expected Lagged with n>=5, got n={n}");
			}
			other => panic!("expected Lagged, got {other:?}"),
		}
	}
}
