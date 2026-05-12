//! `BroadcastSink` — fans `FlowLogEvent`s into a tokio broadcast
//! channel for streaming mgmt verbs (`tail_flow`).
//!
//! Bounded ring; lagging subscribers drop frames rather than back-
//! pressuring the executor. The executor calls `emit` once per event
//! regardless of how many subscribers are connected; broadcast's
//! per-subscriber backlog handling lives in the channel itself.

use tokio::sync::broadcast;
use vane_core::{FlowLogEvent, FlowLogSink};

/// Default flow-log broadcast capacity. 4096 matches the volume
/// profile of `spec/flow-model.md` § _Flow log verbosity_: flow logs
/// run at per-step granularity inside the executor walk so they're
/// significantly higher-volume than tracing frames, and the buffer is
/// sized to absorb a typical-burst worth of events without forcing
/// `Lagged` errors on a freshly-attached `tail_flow` subscriber.
/// Override via `VANE_FLOW_LOG_BROADCAST_CAP`.
///
/// A subscriber that falls more than `capacity` events behind sees
/// [`broadcast::error::RecvError::Lagged`] and resumes from the next
/// available event. The mgmt handler maps that to a synthetic
/// `kind:"lagged"` sentinel so the operator can see they're getting a
/// sampled view rather than the full stream.
pub(crate) const DEFAULT_BROADCAST_CAP: usize = 4096;

/// Env var that overrides [`DEFAULT_BROADCAST_CAP`] at construction
/// time. Empty / unparseable / zero values fall back to the default.
pub(crate) const ENV_BROADCAST_CAP: &str = "VANE_FLOW_LOG_BROADCAST_CAP";

fn resolve_broadcast_cap() -> usize {
	std::env::var(ENV_BROADCAST_CAP)
		.ok()
		.and_then(|s| s.parse::<usize>().ok())
		.filter(|&n| n > 0)
		.unwrap_or(DEFAULT_BROADCAST_CAP)
}

pub struct BroadcastSink {
	tx: broadcast::Sender<FlowLogEvent>,
}

impl BroadcastSink {
	#[must_use]
	pub fn new() -> Self {
		Self::with_capacity(resolve_broadcast_cap())
	}

	/// Explicit-capacity constructor for tests and bespoke wiring. The
	/// `new` / `Default` path resolves from
	/// `VANE_FLOW_LOG_BROADCAST_CAP`, falling back to the crate's
	/// default (currently 4096).
	#[must_use]
	pub fn with_capacity(capacity: usize) -> Self {
		let cap = capacity.max(1);
		let (tx, _initial_rx) = broadcast::channel(cap);
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
		// the steady state when no mgmt client is tailing, BUT it's
		// also indistinguishable from "a subscriber was dropped between
		// `subscribe()` and the next emit". Either way the frame is
		// lost; record one so operators can see the rate of unseen
		// events when nobody's tailing. The counter is sized by the
		// engine's emit rate (high), so be sure the metric has a
		// `reason = "no_subscribers"` slot rather than a label per
		// dropped event.
		if self.tx.send(event).is_err() {
			metrics::counter!(
				"vane.flow_log.broadcast_dropped",
				"reason" => "no_subscribers",
			)
			.increment(1);
		}
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
		let total =
			u32::try_from(DEFAULT_BROADCAST_CAP).expect("DEFAULT_BROADCAST_CAP fits in u32") + 5;
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
