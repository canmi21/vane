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
		// `Close` is phase-agnostic per 05-terminator.md — lower emits it on
		// L4 and L7 paths alike as the default-miss fallback.
		PhaseNodeKind::Terminate(Terminator::Close) => ANY_PHASE,
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

#[cfg(test)]
mod tests {
	use super::*;

	const ALL_PHASES: [Phase; 5] =
		[Phase::L4Raw, Phase::L4Peeked, Phase::L7Request, Phase::L7Response, Phase::Tunnel];

	#[test]
	fn phase_serde_round_trip_per_variant() {
		for p in ALL_PHASES {
			let encoded = serde_json::to_string(&p).expect("serialize");
			let decoded: Phase = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, p);
		}
	}

	#[test]
	fn check_accepts_any_phase() {
		assert_eq!(accepted_in_phases(PhaseNodeKind::Check), ANY_PHASE);
	}

	#[test]
	fn l4_peek_accepts_l4_phases_only() {
		assert_eq!(
			accepted_in_phases(PhaseNodeKind::Middleware(MiddlewareKind::L4Peek)),
			&[Phase::L4Raw, Phase::L4Peeked] as &[Phase],
		);
	}

	#[test]
	fn l4_bytes_accepts_l4_phases_only() {
		assert_eq!(
			accepted_in_phases(PhaseNodeKind::Middleware(MiddlewareKind::L4Bytes)),
			&[Phase::L4Raw, Phase::L4Peeked] as &[Phase],
		);
	}

	#[test]
	fn l7_request_middleware_accepts_only_l7_request() {
		assert_eq!(
			accepted_in_phases(PhaseNodeKind::Middleware(MiddlewareKind::L7Request)),
			&[Phase::L7Request] as &[Phase],
		);
	}

	#[test]
	fn l7_response_middleware_accepts_only_l7_response() {
		assert_eq!(
			accepted_in_phases(PhaseNodeKind::Middleware(MiddlewareKind::L7Response)),
			&[Phase::L7Response] as &[Phase],
		);
	}

	#[test]
	fn upgrade_accepts_only_l4_peeked() {
		assert_eq!(accepted_in_phases(PhaseNodeKind::Upgrade), &[Phase::L4Peeked] as &[Phase]);
	}

	#[test]
	fn l4_forward_fetch_accepts_l4_phases() {
		assert_eq!(
			accepted_in_phases(PhaseNodeKind::Fetch(FetchKind::L4Forward)),
			&[Phase::L4Raw, Phase::L4Peeked] as &[Phase],
		);
	}

	#[test]
	fn http_fetches_accept_only_l7_request() {
		for f in [FetchKind::HttpProxy, FetchKind::HttpSynthesize, FetchKind::WebSocketUpgrade] {
			assert_eq!(accepted_in_phases(PhaseNodeKind::Fetch(f)), &[Phase::L7Request] as &[Phase],);
		}
	}

	#[test]
	fn write_http_response_accepts_only_l7_response() {
		assert_eq!(
			accepted_in_phases(PhaseNodeKind::Terminate(Terminator::WriteHttpResponse)),
			&[Phase::L7Response] as &[Phase],
		);
	}

	#[test]
	fn byte_tunnel_accepts_only_tunnel() {
		assert_eq!(
			accepted_in_phases(PhaseNodeKind::Terminate(Terminator::ByteTunnel)),
			&[Phase::Tunnel] as &[Phase],
		);
	}

	#[test]
	fn check_is_pass_through_at_every_phase() {
		for cur in ALL_PHASES {
			assert_eq!(transition(PhaseNodeKind::Check, cur), Ok(Transition::PassThrough));
		}
	}

	#[test]
	fn l4_peek_forces_out_to_l4_peeked() {
		for cur in [Phase::L4Raw, Phase::L4Peeked] {
			assert_eq!(
				transition(PhaseNodeKind::Middleware(MiddlewareKind::L4Peek), cur),
				Ok(Transition::Into(Phase::L4Peeked)),
			);
		}
	}

	#[test]
	fn l4_bytes_is_pass_through_on_l4_phases() {
		for cur in [Phase::L4Raw, Phase::L4Peeked] {
			assert_eq!(
				transition(PhaseNodeKind::Middleware(MiddlewareKind::L4Bytes), cur),
				Ok(Transition::PassThrough),
			);
		}
	}

	#[test]
	fn upgrade_transitions_l4_peeked_to_l7_request() {
		assert_eq!(
			transition(PhaseNodeKind::Upgrade, Phase::L4Peeked),
			Ok(Transition::Into(Phase::L7Request)),
		);
	}

	#[test]
	fn l7_request_middleware_stays_in_l7_request() {
		assert_eq!(
			transition(PhaseNodeKind::Middleware(MiddlewareKind::L7Request), Phase::L7Request),
			Ok(Transition::Into(Phase::L7Request)),
		);
	}

	#[test]
	fn l7_response_middleware_stays_in_l7_response() {
		assert_eq!(
			transition(PhaseNodeKind::Middleware(MiddlewareKind::L7Response), Phase::L7Response),
			Ok(Transition::Into(Phase::L7Response)),
		);
	}

	#[test]
	fn l4_forward_fetch_goes_to_tunnel_from_any_l4_phase() {
		for cur in [Phase::L4Raw, Phase::L4Peeked] {
			assert_eq!(
				transition(PhaseNodeKind::Fetch(FetchKind::L4Forward), cur),
				Ok(Transition::Into(Phase::Tunnel)),
			);
		}
	}

	#[test]
	fn http_fetch_variants_go_to_l7_response() {
		for f in [FetchKind::HttpProxy, FetchKind::HttpSynthesize] {
			assert_eq!(
				transition(PhaseNodeKind::Fetch(f), Phase::L7Request),
				Ok(Transition::Into(Phase::L7Response)),
			);
		}
	}

	#[test]
	fn websocket_fetch_is_bi_outcome() {
		assert_eq!(
			transition(PhaseNodeKind::Fetch(FetchKind::WebSocketUpgrade), Phase::L7Request),
			Ok(Transition::BiOutcome { response: Phase::L7Response, tunnel: Phase::Tunnel }),
		);
	}

	#[test]
	fn write_http_response_is_terminal() {
		assert_eq!(
			transition(PhaseNodeKind::Terminate(Terminator::WriteHttpResponse), Phase::L7Response),
			Ok(Transition::Terminal),
		);
	}

	#[test]
	fn byte_tunnel_is_terminal() {
		assert_eq!(
			transition(PhaseNodeKind::Terminate(Terminator::ByteTunnel), Phase::Tunnel),
			Ok(Transition::Terminal),
		);
	}

	#[test]
	fn rejects_out_of_phase_attempts() {
		let cases: &[(PhaseNodeKind, Phase)] = &[
			(PhaseNodeKind::Upgrade, Phase::L4Raw),
			(PhaseNodeKind::Upgrade, Phase::L7Request),
			(PhaseNodeKind::Middleware(MiddlewareKind::L7Request), Phase::L4Raw),
			(PhaseNodeKind::Middleware(MiddlewareKind::L7Response), Phase::L7Request),
			(PhaseNodeKind::Fetch(FetchKind::HttpProxy), Phase::L7Response),
			(PhaseNodeKind::Fetch(FetchKind::L4Forward), Phase::L7Request),
			(PhaseNodeKind::Terminate(Terminator::WriteHttpResponse), Phase::Tunnel),
			(PhaseNodeKind::Terminate(Terminator::ByteTunnel), Phase::L7Response),
		];
		for (kind, cur) in cases.iter().copied() {
			let err = transition(kind, cur).expect_err("out-of-phase must error");
			assert_eq!(err.got, cur);
			assert_eq!(err.expected, accepted_in_phases(kind));
		}
	}
}
