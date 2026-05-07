//! Integration tests for `vane_engine::flow_log_sink`.
//!
//! Covers the three concrete sinks (`RingBufferSink`, `FanoutSink`,
//! `FileSink`) plus the daemon-global `VerbosityState` toggle defined in
//! `spec/flow-model.md` § _Flow log verbosity_ /
//! _Default sink composition_:
//!
//! * `RingBufferSink` is a sliding window keyed on event timestamp with
//!   both cap-based and ttl-based eviction.
//! * `FanoutSink` clones each emitted event into every wrapped sink.
//! * `FileSink` is non-blocking (mpsc) and a background task writes
//!   NDJSON lines to disk; flushing on drop preserves enqueued events.
//! * `VerbosityState` starts in `Trajectory` and flips both directions.

#![allow(clippy::too_many_lines)]

use std::sync::Arc;
use std::time::Duration;

use vane_core::{ConnId, FlowLogEvent, FlowLogKind, FlowLogSink, FlowLogVerbosity};
use vane_engine::flow_log_sink::{FanoutSink, FileSink, RingBufferSink};
use vane_engine::verbosity::VerbosityState;

// ---------------------------------------------------------------------------
// Shared helper: build a minimal `FlowLogEvent` whose `t` and `seq` are the
// only varying inputs. The `kind` is fixed at `Trajectory` because none of
// these tests exercise kind-dependent sink behaviour.
// ---------------------------------------------------------------------------

fn make_event(t: u64, seq: u32) -> FlowLogEvent {
	FlowLogEvent {
		t,
		conn: ConnId(0),
		seq,
		kind: FlowLogKind::Trajectory,
		node: None,
		error: None,
		data: None,
	}
}

// ---------------------------------------------------------------------------
// 6. ring_buffer_evicts_on_cap_overflow
// ---------------------------------------------------------------------------

#[test]
fn ring_buffer_evicts_on_cap_overflow() {
	// Spec (02-flow.md § _Default sink composition_): the ring is a sliding
	// window of size `cap`. Emitting more than `cap` events with no ttl
	// pressure must drop the oldest by arrival order.
	let ring = RingBufferSink::new(3, Duration::from_hours(1));
	for seq in 0..5 {
		ring.emit(make_event(0, seq));
	}
	let snap = ring.snapshot();
	assert_eq!(snap.len(), 3, "cap=3 must hold exactly three events");
	let surviving: Vec<u32> = snap.iter().map(|e| e.seq).collect();
	assert_eq!(surviving, vec![2, 3, 4], "the three most-recent pushes survive");
}

// ---------------------------------------------------------------------------
// 7. ring_buffer_evicts_on_ttl_expiry
// ---------------------------------------------------------------------------

#[test]
fn ring_buffer_evicts_on_ttl_expiry() {
	// Spec (02-flow.md § _Default sink composition_): the 60s default ttl
	// is a sliding window keyed on `event.t` differences. We stub a 10ms
	// ttl and emit two events 20ms apart; the elder must be evicted.
	let ring = RingBufferSink::new(10, Duration::from_millis(10));
	ring.emit(make_event(0, 0));
	ring.emit(make_event(20, 1));
	ring.emit(make_event(20, 2));
	let snap = ring.snapshot();
	let surviving: Vec<u32> = snap.iter().map(|e| e.seq).collect();
	assert_eq!(surviving, vec![1, 2], "t=0 must be ttl-evicted; the two t=20 events survive");
}

// ---------------------------------------------------------------------------
// 8. ring_buffer_snapshot_returns_clone_in_order
// ---------------------------------------------------------------------------

#[test]
fn ring_buffer_snapshot_returns_clone_in_order() {
	// Spec (02-flow.md § _Default sink composition_): `snapshot` returns
	// the contents in arrival order so the management API's `tail_flow`
	// can stream them. It must be a clone — mutating the returned vec must
	// not disturb subsequent snapshots.
	let ring = RingBufferSink::with_defaults();
	assert!(ring.is_empty(), "freshly constructed ring is empty");
	for seq in 0..4 {
		ring.emit(make_event(0, seq));
	}
	assert_eq!(ring.len(), 4);
	assert!(!ring.is_empty());

	let mut first = ring.snapshot();
	let second = ring.snapshot();
	let firsts_seqs: Vec<u32> = first.iter().map(|e| e.seq).collect();
	let seconds_seqs: Vec<u32> = second.iter().map(|e| e.seq).collect();
	assert_eq!(firsts_seqs, vec![0, 1, 2, 3]);
	assert_eq!(seconds_seqs, vec![0, 1, 2, 3]);

	// Mutating the first snapshot must not affect the second nor the ring.
	first.clear();
	let third = ring.snapshot();
	let thirds_seqs: Vec<u32> = third.iter().map(|e| e.seq).collect();
	assert_eq!(seconds_seqs, vec![0, 1, 2, 3], "earlier snapshot is independent");
	assert_eq!(thirds_seqs, vec![0, 1, 2, 3], "ring is unaffected by snapshot mutation");
}

// ---------------------------------------------------------------------------
// 9. fanout_emits_to_all_wrapped
// ---------------------------------------------------------------------------

#[test]
fn fanout_emits_to_all_wrapped() {
	// Spec (02-flow.md § _Default sink composition_): the daemon's default
	// sink is a `FanoutSink` of a `RingBufferSink` plus an optional
	// `FileSink`; each wrapped sink must observe every emitted event.
	let a = Arc::new(RingBufferSink::with_defaults());
	let b = Arc::new(RingBufferSink::with_defaults());
	let a_dyn: Arc<dyn FlowLogSink> = a.clone();
	let b_dyn: Arc<dyn FlowLogSink> = b.clone();
	let fan = FanoutSink::new(vec![a_dyn, b_dyn]);
	assert_eq!(fan.len(), 2);
	assert!(!fan.is_empty());

	fan.emit(make_event(1, 7));

	let snap_a = a.snapshot();
	let snap_b = b.snapshot();
	assert_eq!(snap_a.len(), 1, "fanout must reach the first wrapped sink");
	assert_eq!(snap_b.len(), 1, "fanout must reach the second wrapped sink");
	assert_eq!(snap_a[0].seq, 7);
	assert_eq!(snap_b[0].seq, 7);
}

// ---------------------------------------------------------------------------
// 10. file_sink_writes_ndjson_to_path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_sink_writes_ndjson_to_path() {
	// Spec (02-flow.md § _Default sink composition_): `FileSink` is the
	// opt-in NDJSON appender. `emit` is sync and non-blocking; a tokio
	// background task drains an mpsc channel and writes one
	// serde_json-encoded line per event. Drop closes the channel; the
	// writer task drains and flushes before exiting.
	let path = std::env::temp_dir().join(format!("vane-c75-{}.ndjson", std::process::id()));
	// Ensure a clean slate even if an earlier test crashed mid-run.
	let _ = std::fs::remove_file(&path);

	{
		let sink = FileSink::spawn(&path).await.expect("spawn file sink");
		for seq in 0..3 {
			sink.emit(make_event(1_000, seq));
		}
		// Sink drops here → mpsc channel closes → bg task drains.
	}

	// Poll for the writer task to flush. The bg task drains an unbounded
	// channel and flushes per emit, so 200ms is generous.
	let mut elapsed = Duration::from_millis(0);
	let step = Duration::from_millis(20);
	let cap = Duration::from_millis(200);
	let lines = loop {
		let lines = match std::fs::read_to_string(&path) {
			Ok(s) => s.lines().filter(|l| !l.is_empty()).count(),
			Err(_) => 0,
		};
		if lines >= 3 || elapsed >= cap {
			break lines;
		}
		tokio::time::sleep(step).await;
		elapsed += step;
	};
	assert_eq!(lines, 3, "file sink must persist exactly three NDJSON lines (saw {lines})");

	let body = std::fs::read_to_string(&path).expect("read file");
	let parsed: Vec<FlowLogEvent> = body
		.lines()
		.filter(|l| !l.is_empty())
		.map(|l| serde_json::from_str::<FlowLogEvent>(l).expect("decode NDJSON line"))
		.collect();
	assert_eq!(parsed.len(), 3);
	let seqs: Vec<u32> = parsed.iter().map(|e| e.seq).collect();
	assert_eq!(seqs, vec![0, 1, 2]);

	let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// 11. verbosity_state_starts_trajectory_and_set_debug_then_back
// ---------------------------------------------------------------------------

#[test]
fn verbosity_state_starts_trajectory_and_set_debug_then_back() {
	// Spec (02-flow.md § _Flow log verbosity_): the daemon-global
	// `VerbosityState` starts as `Trajectory`. The management API toggle
	// flips it to `Debug` and back; the read is lock-free.
	let state = VerbosityState::new();
	assert_eq!(state.current(), FlowLogVerbosity::Trajectory, "default verbosity is Trajectory");

	state.set(FlowLogVerbosity::Debug);
	assert_eq!(state.current(), FlowLogVerbosity::Debug, "set(Debug) flips the AtomicU8");

	state.set(FlowLogVerbosity::Trajectory);
	assert_eq!(
		state.current(),
		FlowLogVerbosity::Trajectory,
		"set(Trajectory) flips back to default",
	);
}
