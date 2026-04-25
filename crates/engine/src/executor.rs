use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use vane_core::{
	ConnContext, Decision, Error, FlowCtx, FlowLogEvent, FlowLogKind, FlowLogVerbosity, L4Conn, Node,
	NodeId, PredicateView, Request, Response, SerializedError, ShortCircuit, TerminatorOutcomeKind,
	TrajectoryOutcome, TrajectoryStep, Tunnel,
};

use crate::flow_graph::{FetchInst, FlowGraph, MiddlewareInst};

// Both variants are boxed: `L4Conn` embeds a `TcpStream` / `UdpAssoc` and
// `Request` embeds `http::Request<Body>` whose `Body::Stream` variant holds
// a `Pin<Box<dyn HttpBody>>`. Boxing keeps the enum's stack size small and
// symmetric.
pub enum ExecutorInput {
	L4(Box<L4Conn>),
	L7(Box<Request>),
}

/// Iterative walker per 02-flow.md § _Execution model_ + § _Flow log
/// verbosity_. A single async loop holds a `NodeId` cursor and four
/// phase-scoped owned slots; the phase state machine (enforced in core's
/// `validate`) guarantees that at most one slot is `Some` at any point and
/// that `.take().expect("phase invariant")` is sound at each consumption
/// site.
///
/// # Errors
/// Surfaces any middleware / fetch `Err(_)` that isn't routed via a
/// `Node::Middleware.on_error`, plus `Decision::Short(Close)` application-
/// level refusals. Structural stubs — `Upgrade`, body-collect, short-
/// circuit response — return `Error::internal(..)` with a TODO marker
/// naming the chunk that fills them in.
///
/// # Panics
/// `.expect("phase invariant: ...")` calls are sound under a graph that
/// passed core's `validate` pass (02-flow.md § _Phase state machine_ —
/// the phase DFS guarantees each consumer reaches its variant's slot
/// only in the phase that fills it). An engine driving an un-validated
/// or hand-forged graph may hit these; don't.
#[allow(clippy::too_many_lines)]
pub async fn execute(
	graph: &FlowGraph,
	entry: NodeId,
	input: ExecutorInput,
	conn: &Arc<ConnContext>,
	ctx: &mut FlowCtx<'_>,
) -> Result<(), Error> {
	let mut l4: Option<L4Conn> = None;
	let mut req: Option<Request> = None;
	let mut resp: Option<Response> = None;
	let mut tunnel: Option<Tunnel> = None;

	match input {
		ExecutorInput::L4(c) => l4 = Some(*c),
		ExecutorInput::L7(r) => req = Some(*r),
	}

	let mut cur = entry;
	let mut seq: u32 = 0;
	let sym = graph.symbolic();

	loop {
		let node = &sym[cur];

		// Body-collect trigger is a compile-time decision landed by lower's
		// LazyBuffer pass. Real wiring lands at S1-21.
		if node.collect_body_before().is_some() {
			let e = Error::internal(
				"collect_body_before not yet wired — lands with S1-21 middleware that needs body",
			);
			return finish_error(ctx, conn, &mut seq, cur, e);
		}

		match node {
			Node::Check { predicate, on_match, on_miss, .. } => {
				let view = PredicateView::build(conn, req.as_ref(), l4.as_ref());
				let matched = sym[*predicate].test(&view);
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Check, Some(matched));
				cur = if matched { *on_match } else { *on_miss };
			}

			Node::Middleware { id, next, on_error, .. } => {
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Middleware, None);
				let outcome = match &graph[*id] {
					MiddlewareInst::L4Peek(_) => {
						let e =
							Error::internal("L4Peek dispatch deferred — peek buffer wiring lands with S1-16");
						return finish_error(ctx, conn, &mut seq, cur, e);
					}
					MiddlewareInst::L4Bytes(m) => {
						let l4_ref = l4.as_mut().expect("phase invariant: L4Bytes needs L4Conn");
						m.run(l4_ref, conn, ctx).await
					}
					MiddlewareInst::L7Request(m) => {
						let req_ref = req.as_mut().expect("phase invariant: L7Request needs Request");
						m.run(req_ref, conn, ctx).await
					}
					MiddlewareInst::L7Response(m) => {
						let resp_ref = resp.as_mut().expect("phase invariant: L7Response needs Response");
						m.run(resp_ref, conn, ctx).await
					}
				};

				match outcome {
					Ok(Decision::Continue) => cur = *next,
					Ok(Decision::Short(ShortCircuit::Response(_))) => {
						drop(req.take());
						let e = Error::internal(
							"short-circuit response routing deferred — no short_circuit_response_entry metadata yet",
						);
						return finish_error(ctx, conn, &mut seq, cur, e);
					}
					Ok(Decision::Short(ShortCircuit::Close(reason))) => {
						let e = Error::middleware(format!("short-close: {reason:?}"));
						return finish_error(ctx, conn, &mut seq, cur, e);
					}
					Err(e) => {
						emit_error_event(ctx, cur, &mut seq, conn, &e);
						match on_error {
							Some(target) => cur = *target,
							None => return finish_error(ctx, conn, &mut seq, cur, e),
						}
					}
				}
			}

			Node::Fetch { id, next_response, next_tunnel, .. } => {
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Fetch, None);
				match &graph[*id] {
					FetchInst::L7(f) => {
						let r = req.take().expect("phase invariant: L7Fetch needs Request");
						match f.fetch(r, conn, ctx).await {
							Ok(vane_core::L7FetchOutput::Response(rp)) => {
								resp = Some(rp);
								cur = next_response.expect("validator guarantees Some on L7 paths for Response");
							}
							Ok(vane_core::L7FetchOutput::Tunnel(t)) => {
								tunnel = Some(t);
								cur = next_tunnel.expect("validator guarantees Some for WebSocketUpgrade");
							}
							Err(e) => return finish_error(ctx, conn, &mut seq, cur, e),
						}
					}
					FetchInst::L4(f) => {
						let c = l4.take().expect("phase invariant: L4Fetch needs L4Conn");
						match f.fetch(c, conn, ctx).await {
							Ok(t) => {
								tunnel = Some(t);
								cur = next_tunnel.expect("validator guarantees Some on L4 paths");
							}
							Err(e) => return finish_error(ctx, conn, &mut seq, cur, e),
						}
					}
				}
			}

			Node::Upgrade { .. } => {
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Upgrade, None);
				let e = Error::internal(
					"L4→L7 upgrade not yet wired — lands with S1-16 protocol_detect + hyper server integration",
				);
				return finish_error(ctx, conn, &mut seq, cur, e);
			}

			Node::Terminate(tid) => {
				let term = sym[*tid];
				let kind = match term {
					vane_core::Terminator::Close => TerminatorOutcomeKind::Close,
					vane_core::Terminator::WriteHttpResponse => TerminatorOutcomeKind::WriteHttpResponse,
					vane_core::Terminator::ByteTunnel => TerminatorOutcomeKind::ByteTunnel,
				};
				return finish_terminated(ctx, conn, &mut seq, cur, term, kind, &mut resp, &mut tunnel);
			}
		}
	}
}

// --- Step recording -----------------------------------------------------

fn record_step(
	ctx: &mut FlowCtx<'_>,
	conn: &Arc<ConnContext>,
	seq: &mut u32,
	cur: NodeId,
	kind: FlowLogKind,
	branch: Option<bool>,
) {
	// 02-flow.md line 469: one `tracing::trace!` per iter, always on (gated
	// only by RUST_LOG).
	tracing::trace!(node_id = ?cur, kind = ?kind);
	ctx.trajectory.push(TrajectoryStep { node: cur, kind, branch });

	if matches!(ctx.verbosity, FlowLogVerbosity::Debug) {
		ctx.log.emit(FlowLogEvent {
			t: now_ms(),
			conn: conn.id,
			seq: bump(seq),
			kind,
			node: Some(cur),
			error: None,
			data: None,
		});
	}
}

// --- Terminate / Error finalisation ------------------------------------

// Signature kept `Result<(), Error>` for the real terminators that land at
// S1-23 / S1-24 — the write path can fail (client hangup, H2 stream reset,
// etc.) and the executor still needs to propagate. `too_many_arguments` is
// a stylistic warning; bundling these into a struct buys nothing.
#[allow(clippy::too_many_arguments, clippy::unnecessary_wraps)]
fn finish_terminated(
	ctx: &mut FlowCtx<'_>,
	conn: &Arc<ConnContext>,
	seq: &mut u32,
	cur: NodeId,
	term: vane_core::Terminator,
	outcome_kind: TerminatorOutcomeKind,
	resp: &mut Option<Response>,
	tunnel: &mut Option<Tunnel>,
) -> Result<(), Error> {
	// Run the terminator (stub today — drops payload + traces).
	terminate_action(term, cur, resp, tunnel);

	// Connection-level milestone — kept independent of verbosity per
	// 02-flow.md § _Flow log verbosity_: Terminate / Error / Upgrade /
	// SecurityLimit always land in the sink.
	if matches!(term, vane_core::Terminator::Close) {
		ctx.log.emit(FlowLogEvent {
			t: now_ms(),
			conn: conn.id,
			seq: bump(seq),
			kind: FlowLogKind::Terminate,
			node: Some(cur),
			error: None,
			data: Some(serde_json::json!({
				"terminator": "close",
				"reason": "no matching rule",
			})),
		});
	}

	// One Trajectory event per request, always.
	emit_trajectory(
		ctx,
		conn,
		seq,
		TrajectoryOutcome::Terminated { node: cur, terminator: outcome_kind },
	);
	Ok(())
}

fn finish_error(
	ctx: &mut FlowCtx<'_>,
	conn: &Arc<ConnContext>,
	seq: &mut u32,
	cur: NodeId,
	err: Error,
) -> Result<(), Error> {
	let message = std::borrow::Cow::Owned(err.to_string());
	emit_trajectory(ctx, conn, seq, TrajectoryOutcome::Error { node: cur, message });
	Err(err)
}

fn emit_trajectory(
	ctx: &mut FlowCtx<'_>,
	conn: &Arc<ConnContext>,
	seq: &mut u32,
	outcome: TrajectoryOutcome,
) {
	// `ctx.trajectory` is moved out via swap so we can call `finalize`
	// (which consumes by value). Replace with a fresh empty builder so the
	// `FlowCtx` stays in a valid state — same conn, same entry, no steps.
	let conn_id = conn.id;
	let traj = std::mem::replace(
		&mut ctx.trajectory,
		vane_core::TrajectoryBuilder::new(conn_id, NodeId::new(0), now_ms()),
	)
	.finalize(outcome, now_ms());

	let data = serde_json::to_value(&traj).ok();
	ctx.log.emit(FlowLogEvent {
		t: now_ms(),
		conn: conn_id,
		seq: bump(seq),
		kind: FlowLogKind::Trajectory,
		node: None,
		error: None,
		data,
	});
}

fn terminate_action(
	which: vane_core::Terminator,
	cur: NodeId,
	resp: &mut Option<Response>,
	tunnel: &mut Option<Tunnel>,
) {
	match which {
		vane_core::Terminator::Close => {
			// No socket work yet; full close happens once listener owns the
			// transport (S1-13/14).
		}
		// TODO(s1-23): replace stub with hyper response writer.
		vane_core::Terminator::WriteHttpResponse => {
			let _ = resp.take().expect("phase invariant: WriteHttpResponse needs Response");
			tracing::trace!(node_id = ?cur, "stub write_http_response");
		}
		// TODO(s1-24): replace stub with tokio::io::copy_bidirectional.
		vane_core::Terminator::ByteTunnel => {
			let _ = tunnel.take().expect("phase invariant: ByteTunnel needs Tunnel");
			tracing::trace!(node_id = ?cur, "stub byte_tunnel");
		}
	}
}

fn emit_error_event(
	ctx: &mut FlowCtx<'_>,
	cur: NodeId,
	seq: &mut u32,
	conn: &Arc<ConnContext>,
	err: &Error,
) {
	ctx.log.emit(FlowLogEvent {
		t: now_ms(),
		conn: conn.id,
		seq: bump(seq),
		kind: FlowLogKind::Error,
		node: Some(cur),
		error: Some(Arc::new(SerializedError::from(err))),
		data: None,
	});
}

fn bump(seq: &mut u32) -> u32 {
	let n = *seq;
	*seq = seq.saturating_add(1);
	n
}

fn now_ms() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
		.unwrap_or_default()
}
