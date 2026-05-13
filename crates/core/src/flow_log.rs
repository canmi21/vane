use std::sync::Arc;

use crate::conn_context::ConnId;
use crate::error::SerializedError;
use crate::ir::NodeId;

pub trait FlowLogSink: Send + Sync {
	fn emit(&self, event: FlowLogEvent);
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FlowLogEvent {
	pub t: u64,
	pub conn: ConnId,
	pub seq: u32,
	pub kind: FlowLogKind,
	pub node: Option<NodeId>,
	pub error: Option<Arc<SerializedError>>,
	pub data: Option<serde_json::Value>,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum FlowLogKind {
	Check,
	Middleware,
	Fetch,
	Terminate,
	Error,
	SecurityLimit,
	Upgrade,
	/// Per-request summary event. The `data` field carries a serialized
	/// [`FlowTrajectory`]. Always emitted exactly once per request,
	/// regardless of verbosity.
	Trajectory,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum FlowLogVerbosity {
	/// Default. One `Trajectory` event per request, plus the existing
	/// per-connection milestone events (`Terminate`, `Error`, `Upgrade`,
	/// `SecurityLimit`).
	Trajectory,
	/// Adds a per-step event for each `Check` / `Middleware` / `Fetch` /
	/// `Upgrade` node. Used at incident time; not for production volumes.
	Debug,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrajectoryStep {
	pub node: NodeId,
	pub kind: FlowLogKind,
	/// `Some(true)` = Check matched, `Some(false)` = Check missed; `None`
	/// for non-Check steps.
	pub branch: Option<bool>,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum TerminatorOutcomeKind {
	Close,
	WriteHttpResponse,
	ByteTunnel,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum TrajectoryOutcome {
	Terminated { node: NodeId, terminator: TerminatorOutcomeKind },
	Error { node: NodeId, message: TrajectoryErrorMessage },
}

/// Capped error-message payload for [`TrajectoryOutcome::Error`].
///
/// Wraps a `String` that has been truncated to
/// [`TrajectoryErrorMessage::MAX_BYTES`]. `Cow<'static, str>` (the
/// previous shape) made `err.to_string().into()` look harmless even
/// though the full `Display` form can carry many KiB of context —
/// every event then balloons the size of the flow-log sinks. Constrain
/// the type so a caller can't accidentally bypass the cap.
///
/// Construction:
///
/// - `From<&Error>` — the production path: routes the message through
///   [`SerializedError::from`], inheriting its byte cap.
/// - `from_static(&'static str)` — convenience for tests / fixtures.
/// - `from_truncated(String)` — explicit cap on an already-built
///   string; useful when the caller already has a message they don't
///   want to re-wrap.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct TrajectoryErrorMessage(String);

impl TrajectoryErrorMessage {
	/// Hard cap on the rendered message. Matches the
	/// `SerializedError::message` ceiling so the two carriers stay in
	/// lock-step; anything beyond this is truncated with a
	/// `… [truncated]` suffix.
	pub const MAX_BYTES: usize = crate::error::SERIALIZED_MESSAGE_CAP;

	/// Build from a static string slice. No truncation needed at the
	/// type level — call sites that exceed the cap are caller-error.
	#[must_use]
	pub fn from_static(s: &'static str) -> Self {
		Self(cap_message_for_traj(s.to_owned()))
	}

	/// Cap an already-built `String` to [`Self::MAX_BYTES`].
	#[must_use]
	pub fn from_truncated(s: String) -> Self {
		Self(cap_message_for_traj(s))
	}

	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl From<&crate::error::Error> for TrajectoryErrorMessage {
	fn from(err: &crate::error::Error) -> Self {
		Self(SerializedError::from(err).message)
	}
}

fn cap_message_for_traj(s: String) -> String {
	const SUFFIX: &str = "… [truncated]";
	if s.len() <= TrajectoryErrorMessage::MAX_BYTES {
		return s;
	}
	let budget = TrajectoryErrorMessage::MAX_BYTES.saturating_sub(SUFFIX.len());
	let mut end = budget.min(s.len());
	while end > 0 && !s.is_char_boundary(end) {
		end -= 1;
	}
	let mut out = String::with_capacity(end + SUFFIX.len());
	out.push_str(&s[..end]);
	out.push_str(SUFFIX);
	out
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FlowTrajectory {
	pub conn: ConnId,
	pub entry: NodeId,
	pub steps: Vec<TrajectoryStep>,
	pub outcome: TrajectoryOutcome,
	pub started_at_ms: u64,
	pub finished_at_ms: u64,
}

/// Per-walker accumulator that the executor pushes steps into and
/// converts to a [`FlowTrajectory`] at terminate/error time. Not a
/// `FlowLogSink` — the executor explicitly emits one event from the
/// finalized trajectory.
#[derive(Debug)]
pub struct TrajectoryBuilder {
	conn: ConnId,
	entry: NodeId,
	started_at_ms: u64,
	steps: Vec<TrajectoryStep>,
}

impl TrajectoryBuilder {
	#[must_use]
	pub fn new(conn: ConnId, entry: NodeId, started_at_ms: u64) -> Self {
		Self { conn, entry, started_at_ms, steps: Vec::new() }
	}

	/// Detached builder used as a transient placeholder when the
	/// owning [`FlowCtx`](crate::flow_ctx::FlowCtx) needs to swap its
	/// trajectory out via [`std::mem::replace`] (finalize consumes by
	/// value, so the `FlowCtx` must hold *something* in the slot
	/// during the call).
	///
	/// The resulting builder records no entry node and is discarded
	/// immediately after the swap — callers must not push steps to
	/// it or finalize it as if it represented a real trace.
	#[must_use]
	pub fn placeholder(conn: ConnId, started_at_ms: u64) -> Self {
		Self { conn, entry: NodeId::new(0), started_at_ms, steps: Vec::new() }
	}

	pub fn push(&mut self, step: TrajectoryStep) {
		self.steps.push(step);
	}

	#[must_use]
	pub fn finalize(self, outcome: TrajectoryOutcome, finished_at_ms: u64) -> FlowTrajectory {
		FlowTrajectory {
			conn: self.conn,
			entry: self.entry,
			steps: self.steps,
			outcome,
			started_at_ms: self.started_at_ms,
			finished_at_ms,
		}
	}
}

#[cfg(test)]
mod tests {
	use parking_lot::Mutex;

	use super::*;
	use crate::error::Error;

	struct RecordingSink {
		events: Mutex<Vec<FlowLogEvent>>,
	}

	impl FlowLogSink for RecordingSink {
		fn emit(&self, event: FlowLogEvent) {
			self.events.lock().push(event);
		}
	}

	fn sample_event(seq: u32, kind: FlowLogKind) -> FlowLogEvent {
		FlowLogEvent {
			t: 1_234_567_890_123,
			conn: ConnId(0x0bad_f00d_dead_beef),
			seq,
			kind,
			node: Some(NodeId::new(42)),
			error: None,
			data: Some(serde_json::json!({ "kv": "v" })),
		}
	}

	#[test]
	fn flow_log_event_round_trips_through_json() {
		let err = Error::internal("boom");
		let event = FlowLogEvent {
			t: 1_700_000_000_000,
			conn: ConnId(7),
			seq: 13,
			kind: FlowLogKind::Error,
			node: Some(NodeId::new(3)),
			error: Some(Arc::new(SerializedError::from(&err))),
			data: Some(serde_json::json!({ "note": "sample" })),
		};
		let encoded = serde_json::to_string(&event).expect("serialize");
		let decoded: FlowLogEvent = serde_json::from_str(&encoded).expect("deserialize");

		assert_eq!(decoded.t, event.t);
		assert_eq!(decoded.conn, event.conn);
		assert_eq!(decoded.seq, event.seq);
		assert_eq!(decoded.kind, event.kind);
		assert_eq!(decoded.node, event.node);
		assert_eq!(decoded.data, event.data);
		let dec_err = decoded.error.as_ref().expect("error preserved");
		let src_err = event.error.as_ref().expect("error set");
		assert_eq!(dec_err.kind, src_err.kind);
		assert_eq!(dec_err.reason, src_err.reason);
		assert_eq!(dec_err.message, src_err.message);
		assert_eq!(dec_err.ctx, src_err.ctx);
		assert_eq!(dec_err.source_chain, src_err.source_chain);
		assert_eq!(dec_err.http_status, src_err.http_status);
		assert_eq!(dec_err.retryable, src_err.retryable);
	}

	#[test]
	fn flow_log_kind_serde_round_trip_per_variant() {
		for k in [
			FlowLogKind::Check,
			FlowLogKind::Middleware,
			FlowLogKind::Fetch,
			FlowLogKind::Terminate,
			FlowLogKind::Error,
			FlowLogKind::SecurityLimit,
			FlowLogKind::Upgrade,
		] {
			let encoded = serde_json::to_string(&k).expect("serialize");
			let decoded: FlowLogKind = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, k);
		}
	}

	#[test]
	fn flow_log_sink_trait_accepts_concrete_impl_and_records_in_order() {
		let sink = RecordingSink { events: Mutex::new(Vec::new()) };
		let first = sample_event(1, FlowLogKind::Check);
		let second = sample_event(2, FlowLogKind::Middleware);
		sink.emit(first.clone());
		sink.emit(second.clone());
		let recorded = sink.events.lock();
		assert_eq!(recorded.len(), 2);
		assert_eq!(recorded[0].seq, first.seq);
		assert_eq!(recorded[0].kind, first.kind);
		assert_eq!(recorded[1].seq, second.seq);
		assert_eq!(recorded[1].kind, second.kind);
	}

	#[test]
	fn flow_log_sink_is_usable_as_trait_object() {
		let sink = RecordingSink { events: Mutex::new(Vec::new()) };
		// Coerce to trait object and invoke through the vtable; validates
		// that the trait's `fn emit(&self, ...)` signature is object-safe.
		let dyn_sink: &dyn FlowLogSink = &sink;
		dyn_sink.emit(sample_event(1, FlowLogKind::Fetch));
		assert_eq!(sink.events.lock().len(), 1);
	}

	#[test]
	fn trajectory_builder_pushes_in_order_and_finalizes_terminated() {
		let mut b = TrajectoryBuilder::new(ConnId(7), NodeId::new(0), 1_000);
		b.push(TrajectoryStep { node: NodeId::new(1), kind: FlowLogKind::Check, branch: Some(true) });
		b.push(TrajectoryStep { node: NodeId::new(2), kind: FlowLogKind::Middleware, branch: None });
		b.push(TrajectoryStep { node: NodeId::new(3), kind: FlowLogKind::Fetch, branch: None });

		let traj = b.finalize(
			TrajectoryOutcome::Terminated {
				node: NodeId::new(4),
				terminator: TerminatorOutcomeKind::WriteHttpResponse,
			},
			1_500,
		);

		assert_eq!(traj.conn, ConnId(7));
		assert_eq!(traj.entry, NodeId::new(0));
		assert_eq!(traj.started_at_ms, 1_000);
		assert_eq!(traj.finished_at_ms, 1_500);
		assert_eq!(traj.steps.len(), 3);

		assert_eq!(traj.steps[0].node, NodeId::new(1));
		assert_eq!(traj.steps[0].kind, FlowLogKind::Check);
		assert_eq!(traj.steps[0].branch, Some(true));

		assert_eq!(traj.steps[1].node, NodeId::new(2));
		assert_eq!(traj.steps[1].kind, FlowLogKind::Middleware);
		assert_eq!(traj.steps[1].branch, None);

		assert_eq!(traj.steps[2].node, NodeId::new(3));
		assert_eq!(traj.steps[2].kind, FlowLogKind::Fetch);
		assert_eq!(traj.steps[2].branch, None);

		match traj.outcome {
			TrajectoryOutcome::Terminated { node, terminator } => {
				assert_eq!(node, NodeId::new(4));
				assert_eq!(terminator, TerminatorOutcomeKind::WriteHttpResponse);
			}
			other @ TrajectoryOutcome::Error { .. } => {
				panic!("expected Terminated outcome, got {other:?}")
			}
		}
	}

	#[test]
	fn trajectory_builder_finalizes_with_error_outcome() {
		let b = TrajectoryBuilder::new(ConnId(7), NodeId::new(0), 1_000);
		let traj = b.finalize(
			TrajectoryOutcome::Error {
				node: NodeId::new(0),
				message: TrajectoryErrorMessage::from_static("boom"),
			},
			2_000,
		);

		assert!(traj.steps.is_empty(), "no pushes → no steps in finalized trajectory");
		match &traj.outcome {
			TrajectoryOutcome::Error { node, message } => {
				assert_eq!(*node, NodeId::new(0));
				assert_eq!(message.as_str(), "boom");
			}
			other @ TrajectoryOutcome::Terminated { .. } => {
				panic!("expected Error outcome, got {other:?}")
			}
		}
		assert_eq!(traj.finished_at_ms, 2_000);
	}

	fn assert_trajectories_match(a: &FlowTrajectory, b: &FlowTrajectory) {
		assert_eq!(a.conn, b.conn);
		assert_eq!(a.entry, b.entry);
		assert_eq!(a.started_at_ms, b.started_at_ms);
		assert_eq!(a.finished_at_ms, b.finished_at_ms);
		assert_eq!(a.steps.len(), b.steps.len());
		for (left, right) in a.steps.iter().zip(b.steps.iter()) {
			assert_eq!(left.node, right.node);
			assert_eq!(left.kind, right.kind);
			assert_eq!(left.branch, right.branch);
		}
		match (&a.outcome, &b.outcome) {
			(
				TrajectoryOutcome::Terminated { node: na, terminator: ta },
				TrajectoryOutcome::Terminated { node: nb, terminator: tb },
			) => {
				assert_eq!(na, nb);
				assert_eq!(ta, tb);
			}
			(
				TrajectoryOutcome::Error { node: na, message: ma },
				TrajectoryOutcome::Error { node: nb, message: mb },
			) => {
				assert_eq!(na, nb);
				assert_eq!(ma.as_str(), mb.as_str());
			}
			(left, right) => panic!("outcome variant mismatch: {left:?} vs {right:?}"),
		}
	}

	#[test]
	fn flow_trajectory_round_trips_through_json() {
		let mut b = TrajectoryBuilder::new(ConnId(0x1234_5678), NodeId::new(0), 100);
		b.push(TrajectoryStep { node: NodeId::new(1), kind: FlowLogKind::Check, branch: Some(false) });
		b.push(TrajectoryStep { node: NodeId::new(2), kind: FlowLogKind::Upgrade, branch: None });
		let term = b.finalize(
			TrajectoryOutcome::Terminated {
				node: NodeId::new(3),
				terminator: TerminatorOutcomeKind::ByteTunnel,
			},
			200,
		);
		let encoded = serde_json::to_string(&term).expect("serialize terminated");
		let decoded: FlowTrajectory = serde_json::from_str(&encoded).expect("deserialize terminated");
		assert_trajectories_match(&term, &decoded);

		let err = TrajectoryBuilder::new(ConnId(42), NodeId::new(7), 0).finalize(
			TrajectoryOutcome::Error {
				node: NodeId::new(8),
				message: TrajectoryErrorMessage::from_static("upstream went away"),
			},
			17,
		);
		let encoded = serde_json::to_string(&err).expect("serialize error");
		let decoded: FlowTrajectory = serde_json::from_str(&encoded).expect("deserialize error");
		assert_trajectories_match(&err, &decoded);
	}

	#[test]
	fn flow_log_kind_trajectory_serde_round_trip() {
		let encoded = serde_json::to_string(&FlowLogKind::Trajectory).expect("serialize");
		let decoded: FlowLogKind = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, FlowLogKind::Trajectory);
	}

	#[test]
	fn flow_log_verbosity_serde_round_trip_per_variant() {
		for v in [FlowLogVerbosity::Trajectory, FlowLogVerbosity::Debug] {
			let encoded = serde_json::to_string(&v).expect("serialize");
			let decoded: FlowLogVerbosity = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, v);
		}
	}
}
