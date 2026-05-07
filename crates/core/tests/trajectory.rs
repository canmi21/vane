//! Integration tests for `vane_core::flow_log` trajectory types.
//!
//! Covers the `FlowTrajectory` shape and `TrajectoryBuilder` contract
//! defined in `spec/flow-model.md` § _Flow log verbosity_:
//! `TrajectoryBuilder` accumulates `TrajectoryStep`s in push order and
//! `finalize` snapshots them into a `FlowTrajectory` whose outcome is
//! either `Terminated` or `Error`. Also exercises the `serde` round-trip
//! contract on `FlowLogKind::Trajectory` and `FlowLogVerbosity` — both
//! types ship `Serialize + Deserialize` derives that the management API
//! relies on.

use std::borrow::Cow;

use vane_core::{
	ConnId, FlowLogKind, FlowLogVerbosity, FlowTrajectory, NodeId, TerminatorOutcomeKind,
	TrajectoryBuilder, TrajectoryOutcome, TrajectoryStep,
};

#[test]
fn trajectory_builder_pushes_in_order_and_finalizes_terminated() {
	// Spec (02-flow.md § _Flow log verbosity_): `TrajectoryBuilder::push`
	// appends in order; `finalize` snapshots into a `FlowTrajectory` whose
	// `steps` preserve push order, with `started_at_ms` / `finished_at_ms`
	// taken from `new` and `finalize` respectively.
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
	// Spec (02-flow.md § _Flow log verbosity_): the error path finalizes
	// with `TrajectoryOutcome::Error { node, message }`. An empty step list
	// is a valid "errored before any step ran" trajectory.
	let b = TrajectoryBuilder::new(ConnId(7), NodeId::new(0), 1_000);
	let traj = b.finalize(
		TrajectoryOutcome::Error { node: NodeId::new(0), message: Cow::Borrowed("boom") },
		2_000,
	);

	assert!(traj.steps.is_empty(), "no pushes → no steps in finalized trajectory");
	match &traj.outcome {
		TrajectoryOutcome::Error { node, message } => {
			assert_eq!(*node, NodeId::new(0));
			assert_eq!(message.as_ref(), "boom");
		}
		other @ TrajectoryOutcome::Terminated { .. } => {
			panic!("expected Error outcome, got {other:?}")
		}
	}
	assert_eq!(traj.finished_at_ms, 2_000);
}

fn assert_trajectories_match(a: &FlowTrajectory, b: &FlowTrajectory) {
	// `FlowTrajectory` doesn't impl `PartialEq`, so compare field by field.
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
			assert_eq!(ma.as_ref(), mb.as_ref());
		}
		(left, right) => panic!("outcome variant mismatch: {left:?} vs {right:?}"),
	}
}

#[test]
fn flow_trajectory_round_trips_through_json() {
	// Spec (02-flow.md § _Flow log verbosity_): `FlowTrajectory` derives
	// `Serialize + Deserialize`; every field round-trips through serde_json.
	// Cover both `TrajectoryOutcome` variants in the same test.

	// Sub-case A: Terminated outcome with a populated step list.
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

	// Sub-case B: Error outcome with an empty step list.
	let err = TrajectoryBuilder::new(ConnId(42), NodeId::new(7), 0).finalize(
		TrajectoryOutcome::Error { node: NodeId::new(8), message: Cow::Borrowed("upstream went away") },
		17,
	);
	let encoded = serde_json::to_string(&err).expect("serialize error");
	let decoded: FlowTrajectory = serde_json::from_str(&encoded).expect("deserialize error");
	assert_trajectories_match(&err, &decoded);
}

#[test]
fn flow_log_kind_trajectory_serde_round_trip() {
	// `FlowLogKind::Trajectory` is the new variant gating per-request
	// summary events (02-flow.md § _Flow log verbosity_). It must round-
	// trip through serde so the management API can transmit it verbatim.
	let encoded = serde_json::to_string(&FlowLogKind::Trajectory).expect("serialize");
	let decoded: FlowLogKind = serde_json::from_str(&encoded).expect("deserialize");
	assert_eq!(decoded, FlowLogKind::Trajectory);
}

#[test]
fn flow_log_verbosity_serde_round_trip_per_variant() {
	// Both verbosity variants round-trip — the management API toggle wire
	// form depends on the `Serialize + Deserialize` derive (02-flow.md §
	// _Flow log verbosity_).
	for v in [FlowLogVerbosity::Trajectory, FlowLogVerbosity::Debug] {
		let encoded = serde_json::to_string(&v).expect("serialize");
		let decoded: FlowLogVerbosity = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, v);
	}
}
