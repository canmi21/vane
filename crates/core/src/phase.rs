use crate::fetch::{FetchKind, Terminator};
use crate::middleware::MiddlewareKind;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Phase {
	L4Raw,
	L4Peeked,
	L7Request,
	L7Response,
	Tunnel,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum PhaseNodeKind {
	Check,
	Middleware(MiddlewareKind),
	Upgrade,
	Fetch(FetchKind),
	Terminate(Terminator),
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Transition {
	PassThrough,
	Into(Phase),
	BiOutcome { response: Phase, tunnel: Phase },
	Terminal,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct PhaseError {
	pub expected: &'static [Phase],
	pub got: Phase,
}

const L4_ANY: &[Phase] = &[Phase::L4Raw, Phase::L4Peeked];
const L7_REQ: &[Phase] = &[Phase::L7Request];
const L7_RESP: &[Phase] = &[Phase::L7Response];
const L4_PEEKED: &[Phase] = &[Phase::L4Peeked];
const TUNNEL: &[Phase] = &[Phase::Tunnel];
const ANY_PHASE: &[Phase] =
	&[Phase::L4Raw, Phase::L4Peeked, Phase::L7Request, Phase::L7Response, Phase::Tunnel];

// Each arm mirrors one row of the 02-flow.md § _Transition table_. Merging
// arms with equal bodies would hide the table structure, which the spec
// calls out as the whole point of the single-source design.
#[must_use]
#[allow(clippy::match_same_arms)]
pub const fn accepted_in_phases(kind: PhaseNodeKind) -> &'static [Phase] {
	match kind {
		PhaseNodeKind::Check => ANY_PHASE,
		PhaseNodeKind::Middleware(MiddlewareKind::L4Peek) => L4_ANY,
		PhaseNodeKind::Middleware(MiddlewareKind::L4Bytes) => L4_ANY,
		PhaseNodeKind::Middleware(MiddlewareKind::L7Request) => L7_REQ,
		PhaseNodeKind::Middleware(MiddlewareKind::L7Response) => L7_RESP,
		PhaseNodeKind::Upgrade => L4_PEEKED,
		PhaseNodeKind::Fetch(FetchKind::L4Forward) => L4_ANY,
		PhaseNodeKind::Fetch(FetchKind::HttpProxy) => L7_REQ,
		PhaseNodeKind::Fetch(FetchKind::HttpSynthesize) => L7_REQ,
		PhaseNodeKind::Fetch(FetchKind::WebSocketUpgrade) => L7_REQ,
		PhaseNodeKind::Terminate(Terminator::WriteHttpResponse) => L7_RESP,
		PhaseNodeKind::Terminate(Terminator::ByteTunnel) => TUNNEL,
	}
}

/// Look up the out-phase for a node at its current in-phase.
///
/// # Errors
/// Returns [`PhaseError`] when `cur` is not in the node's accepted in-phase
/// set. Validator consumers translate this into the 02-flow.md error format.
#[allow(clippy::match_same_arms)]
pub fn transition(kind: PhaseNodeKind, cur: Phase) -> Result<Transition, PhaseError> {
	let accepted = accepted_in_phases(kind);
	if !accepted.contains(&cur) {
		return Err(PhaseError { expected: accepted, got: cur });
	}
	Ok(match kind {
		PhaseNodeKind::Check => Transition::PassThrough,
		PhaseNodeKind::Middleware(MiddlewareKind::L4Peek) => Transition::Into(Phase::L4Peeked),
		PhaseNodeKind::Middleware(MiddlewareKind::L4Bytes) => Transition::PassThrough,
		PhaseNodeKind::Middleware(MiddlewareKind::L7Request) => Transition::Into(Phase::L7Request),
		PhaseNodeKind::Middleware(MiddlewareKind::L7Response) => Transition::Into(Phase::L7Response),
		PhaseNodeKind::Upgrade => Transition::Into(Phase::L7Request),
		PhaseNodeKind::Fetch(FetchKind::L4Forward) => Transition::Into(Phase::Tunnel),
		PhaseNodeKind::Fetch(FetchKind::HttpProxy) => Transition::Into(Phase::L7Response),
		PhaseNodeKind::Fetch(FetchKind::HttpSynthesize) => Transition::Into(Phase::L7Response),
		PhaseNodeKind::Fetch(FetchKind::WebSocketUpgrade) => {
			Transition::BiOutcome { response: Phase::L7Response, tunnel: Phase::Tunnel }
		}
		PhaseNodeKind::Terminate(_) => Transition::Terminal,
	})
}
