use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use vane_core::{
	ConnContext, Decision, Error, FlowCtx, FlowLogEvent, FlowLogKind, L4Conn, Node, NodeId,
	PredicateView, Request, Response, SerializedError, ShortCircuit, Terminator, Tunnel,
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

/// Iterative walker per 02-flow.md § _Execution model_. A single async loop
/// holds a `NodeId` cursor and four phase-scoped owned slots; the phase
/// state machine (enforced in core's `validate`) guarantees that at most
/// one slot is `Some` at any point and that `.take().expect("phase
/// invariant")` is sound at each consumption site.
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
		// LazyBuffer pass (see 02-flow.md § _LazyBuffer_). Wiring the
		// actual `Body::collect().await` lands with the first middleware
		// that sets the flag (S1-21).
		if node.collect_body_before().is_some() {
			return Err(Error::internal(
				"collect_body_before not yet wired — lands with S1-21 middleware that needs body",
			));
		}

		match node {
			Node::Check { predicate, on_match, on_miss, .. } => {
				trace_step(ctx, cur, &mut seq, "check", conn);
				let view = PredicateView::build(conn, req.as_ref(), l4.as_ref());
				let matched = sym[*predicate].test(&view);
				cur = if matched { *on_match } else { *on_miss };
			}

			Node::Middleware { id, next, on_error, .. } => {
				trace_step(ctx, cur, &mut seq, "mid", conn);
				let outcome = match &graph[*id] {
					// L4Peek dispatch needs the peek buffer on ConnContext —
					// that wiring lands with `protocol_detect` (S1-16). Until
					// then we refuse fast rather than pass an empty slice and
					// silently look matched.
					MiddlewareInst::L4Peek(_) => {
						return Err(Error::internal(
							"L4Peek dispatch deferred — peek buffer wiring lands with S1-16",
						));
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
						// TODO(s1-22): Ok(Short(Response)) should jump to
						// `graph.meta.short_circuit_response_entry(entry)` so
						// response-phase middleware still runs. The helper
						// doesn't exist yet — defer.
						drop(req.take());
						return Err(Error::internal(
							"short-circuit response routing deferred — no short_circuit_response_entry metadata yet",
						));
					}
					Ok(Decision::Short(ShortCircuit::Close(reason))) => {
						return Err(Error::middleware(format!("short-close: {reason:?}")));
					}
					Err(e) => {
						emit_error_event(ctx, cur, &mut seq, conn, &e);
						match on_error {
							Some(target) => cur = *target,
							// TODO(s1-late): route through
							// `graph.meta.default_fallback(phase)` once lower
							// synthesizes per-phase fallback tombstones (spec
							// 02-flow.md § _Execution model_ line 403).
							None => return Err(e),
						}
					}
				}
			}

			Node::Fetch { id, next_response, next_tunnel, .. } => {
				trace_step(ctx, cur, &mut seq, "fetch", conn);
				match &graph[*id] {
					FetchInst::L7(f) => {
						let r = req.take().expect("phase invariant: L7Fetch needs Request");
						match f.fetch(r, conn, ctx).await? {
							vane_core::L7FetchOutput::Response(rp) => {
								resp = Some(rp);
								cur = next_response.expect("validator guarantees Some on L7 paths for Response");
							}
							vane_core::L7FetchOutput::Tunnel(t) => {
								tunnel = Some(t);
								cur = next_tunnel.expect("validator guarantees Some for WebSocketUpgrade");
							}
						}
					}
					FetchInst::L4(f) => {
						let c = l4.take().expect("phase invariant: L4Fetch needs L4Conn");
						let t = f.fetch(c, conn, ctx).await?;
						tunnel = Some(t);
						cur = next_tunnel.expect("validator guarantees Some on L4 paths");
					}
				}
			}

			Node::Upgrade { .. } => {
				trace_step(ctx, cur, &mut seq, "upgrade", conn);
				// TODO(s1-16/s1-17): hand L4Conn to hyper::server and respawn
				// `execute` per decoded Request. This is the L4→L7 boundary;
				// the pseudocode in 02-flow.md references
				// `spawn_http_server(l4, graph, *next, conn.clone())`.
				return Err(Error::internal(
					"L4→L7 upgrade not yet wired — lands with S1-16 protocol_detect + hyper server integration",
				));
			}

			Node::Terminate(tid) => {
				trace_step(ctx, cur, &mut seq, "terminate", conn);
				return terminate(sym[*tid], conn, ctx, &mut seq, cur, &mut resp, &mut tunnel);
			}
		}
	}
}

// Signature keeps `Result<(), Error>` for the real terminators that land at
// S1-23 / S1-24 — the write path can fail (client hangup, H2 stream reset,
// etc.) and the executor still needs to propagate.
#[allow(clippy::unnecessary_wraps)]
fn terminate(
	which: Terminator,
	conn: &Arc<ConnContext>,
	ctx: &mut FlowCtx<'_>,
	seq: &mut u32,
	cur: NodeId,
	resp: &mut Option<Response>,
	tunnel: &mut Option<Tunnel>,
) -> Result<(), Error> {
	match which {
		Terminator::Close => {
			// Silent drop — emit a Terminate event so operators see the
			// traffic did reach the daemon. Full `CloseReason::PolicyDenied`
			// payload lands on the event's `data` field rather than on the
			// transport; there's no socket-level work yet.
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
			Ok(())
		}
		// TODO(s1-23): replace stub with hyper response writer.
		Terminator::WriteHttpResponse => {
			let _ = resp.take().expect("phase invariant: WriteHttpResponse needs Response");
			tracing::trace!(node_id = ?cur, "stub write_http_response");
			Ok(())
		}
		// TODO(s1-24): replace stub with tokio::io::copy_bidirectional.
		Terminator::ByteTunnel => {
			let _ = tunnel.take().expect("phase invariant: ByteTunnel needs Tunnel");
			tracing::trace!(node_id = ?cur, "stub byte_tunnel");
			Ok(())
		}
	}
}

fn trace_step(
	ctx: &mut FlowCtx<'_>,
	cur: NodeId,
	seq: &mut u32,
	kind: &'static str,
	conn: &Arc<ConnContext>,
) {
	// Spec 02-flow.md line 469: one `tracing::trace!` event per loop iter.
	// We additionally mirror the step into `ctx.log` (via the appropriate
	// FlowLogKind) for Check / Middleware / Fetch / Terminate / Upgrade —
	// management-API consumers read the same stream.
	tracing::trace!(node_id = ?cur, kind = kind);
	let flow_kind = match kind {
		"check" => FlowLogKind::Check,
		"mid" => FlowLogKind::Middleware,
		"fetch" => FlowLogKind::Fetch,
		"terminate" => FlowLogKind::Terminate,
		"upgrade" => FlowLogKind::Upgrade,
		_ => return,
	};
	ctx.log.emit(FlowLogEvent {
		t: now_ms(),
		conn: conn.id,
		seq: bump(seq),
		kind: flow_kind,
		node: Some(cur),
		error: None,
		data: None,
	});
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
