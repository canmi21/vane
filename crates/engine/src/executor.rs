use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use vane_core::{
	Body, BodySide, BytesView, CloseReason, ConnContext, ContextEntry, Decision, Error, FlowCtx,
	FlowLogEvent, FlowLogKind, FlowLogVerbosity, Header, L4BytesDecision, L4BytesInput, L4Conn,
	L4PeekDecision, L4PeekInput, L7RequestDecision, L7RequestInput, L7ResponseDecision,
	L7ResponseInput, MiddlewareKind, Node, NodeId, PluginError, PredicateView, Request, Response,
	SerializedError, ShortCircuit, SynthResponse, TerminatorOutcomeKind, TrajectoryOutcome,
	TrajectoryStep, Tunnel, UpstreamReason,
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

/// What the walker hands back to the listener / hyper service-fn that
/// drove `execute`.
///
/// The split exists because each terminator has a different "what's left
/// to do" answer: `Close` is fully done, `ByteTunnel` already drove the
/// copy in-executor, but `WriteHttpResponse` needs the caller to serialise
/// the `Response` onto a socket (the hyper service-fn returns it from the
/// H1 / H2 handler; H3 is the same shape). The executor is socket-free in
/// the L7 path so it stays composable with hyper's request-response model
/// and the future `h3::server` analogue.
pub enum ExecutorOutput {
	/// `Terminator::Close` walked, or any path the executor finalised
	/// without producing a response or tunnel. Caller does nothing
	/// further; transport drop-glue closes.
	Closed,
	/// `Terminator::WriteHttpResponse` walked. Caller serialises this
	/// `Response` to the client socket (hyper / h3).
	HttpResponse(Response),
	/// `Terminator::ByteTunnel` walked. Executor already drove
	/// `tokio::io::copy_bidirectional` to completion; the close reason
	/// (graceful or io-error) was sent through `Tunnel.close_reason_tx`.
	/// Caller does nothing further.
	Tunneled,
}

// Manual `Debug` because `Response` (i.e. `http::Response<Body>`) doesn't
// derive Debug — `Body::Stream(Pin<Box<dyn HttpBody>>)` has no Debug. We
// only need the variant name for `Result::expect_err` / `assert!` debug
// formatting in tests.
impl std::fmt::Debug for ExecutorOutput {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Closed => f.write_str("ExecutorOutput::Closed"),
			Self::HttpResponse(r) => {
				write!(f, "ExecutorOutput::HttpResponse(status={})", r.status().as_u16())
			}
			Self::Tunneled => f.write_str("ExecutorOutput::Tunneled"),
		}
	}
}

/// Iterative walker per spec/flow-model.md § _Executor_ + § _Flow log
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
/// passed core's `validate` pass (spec/flow-model.md § _Phase state machine_ —
/// the phase DFS guarantees each consumer reaches its variant's slot
/// only in the phase that fills it). An engine driving an un-validated
/// or hand-forged graph may hit these; don't.
#[allow(
	clippy::too_many_lines,
	reason = "central FlowGraph state machine: the loop dispatches on Node::{Check,Middleware,Fetch,Upgrade,Terminate} and each arm shares the same mutable l4/req/resp/tunnel/cur/seq locals. Splitting by node kind would need every helper to take 7+ &mut refs (or a giant ctx struct that just renames the noise), which moves complexity rather than reducing it"
)]
pub async fn execute(
	graph: &Arc<FlowGraph>,
	entry: NodeId,
	input: ExecutorInput,
	conn: &Arc<ConnContext>,
	ctx: &mut FlowCtx,
) -> Result<ExecutorOutput, Error> {
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

	// Pull the L4 peek buffer (set by the listener-side prelude on
	// `ConnContext.user`) once for the lifetime of this execute call.
	// Per `spec/crates/engine.md` § _Protocol detection_, the
	// `PeekResult` is immutable after the prelude runs — it never
	// disappears mid-flow — so re-locking on every `Check` / `L4Peek`
	// step (the prior shape) was pure overhead. `bytes::Bytes` clone
	// is a refcount bump; the slice handed to predicate views below
	// borrows from this local. See `H14` in the perf audit.
	let peek_bytes: Option<bytes::Bytes> =
		conn.user.lock().get::<vane_core::PeekResult>().map(|r| r.buffer.clone());

	loop {
		let node = &sym[cur];

		if let Some(side) = node.collect_body_before() {
			let limit = node.body_limit();
			match side {
				BodySide::Request => {
					if let Some(r) = req.as_mut() {
						match collect_body(r.body_mut(), limit).await {
							Ok(()) => {}
							Err(CollectError::TooLarge) => {
								let over_limit_resp = http::Response::builder()
									.status(413)
									.header("connection", "close")
									.body(Body::Empty)
									.expect("static 413 response");
								drop(req.take());
								let target_opt =
									graph.symbolic().meta.short_circuit_response_entry.get(&entry).copied();
								let Some(target) = target_opt else {
									let e = Error::internal("body limit exceeded: no synth target for 413 response");
									return finish_error(ctx, conn, &mut seq, cur, e);
								};
								resp = Some(over_limit_resp);
								cur = target;
								continue;
							}
							Err(CollectError::Io(e)) => {
								return finish_error(ctx, conn, &mut seq, cur, e);
							}
						}
					}
				}
				BodySide::Response => {
					if let Some(r) = resp.as_mut() {
						match collect_body(r.body_mut(), limit).await {
							Ok(()) => {}
							Err(CollectError::TooLarge) => {
								let e = Error::upstream(UpstreamReason::Malformed)
									.with_ctx("response body exceeded max_body_bytes limit");
								return finish_error(ctx, conn, &mut seq, cur, e);
							}
							Err(CollectError::Io(e)) => {
								return finish_error(ctx, conn, &mut seq, cur, e);
							}
						}
					}
				}
			}
		}

		match node {
			Node::Check { predicate, on_match, on_miss, .. } => {
				// `peek_bytes` was captured once at execute entry — see
				// the comment there. The slice handed to the predicate
				// view borrows from the outer local so no per-step lock
				// is needed.
				let view = PredicateView::build(conn, req.as_ref(), l4.as_ref(), peek_bytes.as_deref());
				let matched = sym[*predicate].test(&view);
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Check, Some(matched));
				cur = if matched { *on_match } else { *on_miss };
			}

			Node::Middleware { id, next, on_error, .. } => {
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Middleware, None);
				let outcome = match &graph[*id] {
					MiddlewareInst::L4Peek(m) => {
						// `peek_bytes` was captured once at execute entry. The
						// middleware borrows from that local instead of
						// re-locking `conn.user` per dispatch.
						if peek_bytes.is_none() {
							tracing::warn!(
								conn_id = %conn.id,
								"L4Peek dispatched without PeekResult — listener prelude must run first",
							);
						}
						let peek_slice: &[u8] = peek_bytes.as_deref().unwrap_or(&[]);
						m.run(peek_slice, conn, ctx).await
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
					MiddlewareInst::Wasm(w) => dispatch_wasm(w, &mut l4, &mut req, &mut resp, conn).await,
				};

				match outcome {
					Ok(Decision::Continue) => cur = *next,
					Ok(Decision::Short(ShortCircuit::Response(r))) => {
						// spec/flow-model.md § _Executor_: an L7 request
						// middleware that returns `Short(Response)` parks
						// the response in `resp` and jumps to the
						// listener-level synth `Terminate(WriteHttpResponse)`
						// installed by the lower pass (see § _The compiled form_). The `request` slot is dropped because
						// the L7 chain is bypassed; the synth terminator's
						// caller-writes path emits the pre-built response
						// verbatim.
						//
						// Spec method `short_circuit_response_entry(entry)`
						// is documented as panicking via
						// `expect("lower invariant: ...")`. We use a
						// fallible `get` + `Error::internal` instead: lower
						// is best-effort sync and a missing map entry
						// should propagate as a typed error (caught by the
						// H1 service-fn → 500), not a panic that kills the
						// whole accept loop. Spec deviation flagged.
						drop(req.take());
						let target_opt =
							graph.symbolic().meta.short_circuit_response_entry.get(&entry).copied();
						let Some(target) = target_opt else {
							let e = Error::internal(format!(
								"short-circuit response: entry NodeId({}) has no synth target — lower invariant violated (L7 entry without WriteHttpResponse synth)",
								entry.get(),
							));
							return finish_error(ctx, conn, &mut seq, cur, e);
						};
						resp = Some(r);
						record_step(ctx, conn, &mut seq, cur, FlowLogKind::Middleware, None);
						cur = target;
					}
					Ok(Decision::Short(ShortCircuit::Close(reason))) => {
						// Route by CloseReason variant: routing-level refusals
						// (PolicyDenied / Graceful / Cancelled) are not errors
						// — hand back to the caller as `Ok(Closed)`. The H1
						// service-fn maps that to 404 + `Connection: close`
						// (see `spec/flow-model.md` § _`Terminator::Close` — wire-level manifestation_); the L4 listener drops the
						// socket. Only ProtocolError represents a genuine
						// anomaly that should surface as 500.
						match reason {
							CloseReason::PolicyDenied(_) | CloseReason::Graceful | CloseReason::Cancelled => {
								drop((l4.take(), req.take(), resp.take(), tunnel.take()));
								// Connection-level Terminate milestone — same shape
								// as the `Terminator::Close` arm so the wire-level
								// view is uniform whether the close was a synth
								// default-miss or a middleware short-circuit.
								let reason_text: std::borrow::Cow<'static, str> = match &reason {
									CloseReason::PolicyDenied(s) => s.clone(),
									CloseReason::Graceful => std::borrow::Cow::Borrowed("graceful"),
									CloseReason::Cancelled => std::borrow::Cow::Borrowed("cancelled"),
									CloseReason::ProtocolError(_) => unreachable!(),
								};
								ctx.log.emit(FlowLogEvent {
									t: now_ms(),
									conn: conn.id,
									seq: bump(&mut seq),
									kind: FlowLogKind::Terminate,
									node: Some(cur),
									error: None,
									data: Some(serde_json::json!({
										"terminator": "short_close",
										"reason": reason_text,
									})),
								});
								emit_trajectory(
									ctx,
									conn,
									&mut seq,
									TrajectoryOutcome::Terminated {
										node: cur,
										terminator: TerminatorOutcomeKind::Close,
									},
								);
								return Ok(ExecutorOutput::Closed);
							}
							CloseReason::ProtocolError(_) => {
								let e = Error::middleware(format!("short-close: {reason:?}"));
								return finish_error(ctx, conn, &mut seq, cur, e);
							}
						}
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
						// TLS 1.3 0-RTT (early data) gate. Per
						// `spec/crates/engine-tls.md` § _TLS 1.3 0-RTT (early data)_
						// § _Configuration_, a request that arrived as
						// 0-RTT data and matched a rule with
						// `allow_zero_rtt: false` must receive a
						// synthetic 425 Too Early instead of being
						// handed to the rule's terminator. The
						// connection itself stays open (a well-behaved
						// client retries the request after a full
						// handshake per RFC 8470).
						//
						// `zero_rtt_used` is populated by `run_tls`
						// from rustls's `ServerConnection::early_data`
						// state at handshake completion;
						// `allow_zero_rtt` is lifted off the parent
						// rule onto the symbolic fetch ref by the
						// lower pass.
						//
						// The flag is consumed on read: only the first
						// request after the early-data drain sees it
						// as true. Subsequent requests on a kept-alive
						// HTTP/1.1 connection (or H2 streams that
						// follow the early-data drain) arrive purely
						// as 1-RTT data, so the 425 gate must not fire
						// for them. This matches `spec/crates/engine-tls.md` § _TLS
						// 1.3 0-RTT (early data)_ § _Configuration_,
						// which scopes the gate per request.
						let zero_rtt_used = {
							let mut guard = conn.tls.lock();
							guard.as_mut().is_some_and(|t| std::mem::take(&mut t.zero_rtt_used))
						};
						let allow_zero_rtt = graph.symbolic().fetches[id.get() as usize].allow_zero_rtt;
						if zero_rtt_used && allow_zero_rtt == Some(false) {
							let _ = req.take();
							let response = http::Response::builder()
								.status(http::StatusCode::TOO_EARLY)
								.header(http::header::CACHE_CONTROL, "no-store")
								.body(Body::Empty)
								.expect("static 425");
							resp = Some(response);
							cur = next_response.expect("validator guarantees Some on L7 paths for Response");
							continue;
						}

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

			Node::Upgrade { next } => {
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Upgrade, None);
				// Hand the L4 connection to the H1 or H2 server. Each decoded
				// request constructs a fresh `FlowCtx` and re-enters `execute`
				// from `*next`. See spec/flow-model.md § _Executor_ (Upgrade arm).
				//
				// Plain TCP, TLS-terminated H1, and TLS-terminated H2 all feed
				// the generic stream drivers; the listener has already consumed
				// the TLS handshake (when applicable) and populated
				// `ConnContext.tls` and `conn.http_version` from the negotiated
				// ALPN. We re-read ALPN here so the dispatch is local to this
				// arm rather than spread across the L4Conn variants.
				let l4 = l4.take().expect("phase invariant: Upgrade needs L4Conn");
				// Box each variant uniformly so both drivers see the same
				// trait-object IO type.
				let stream: Box<dyn vane_core::AsyncReadWrite + Send + 'static> = match l4 {
					L4Conn::Tcp(s) => Box::new(s),
					L4Conn::Peeked(s) | L4Conn::Tls(s) => s,
					L4Conn::Udp(_) => {
						let e = Error::internal(
							"UDP upgrade not supported in S1 — QUIC integration lands with H3 / S2",
						);
						return finish_error(ctx, conn, &mut seq, cur, e);
					}
				};
				let alpn = conn.tls.lock().as_ref().and_then(|t| t.alpn.clone());
				// Two signals can pick H2: a negotiated `h2` ALPN (TLS path)
				// or a pre-set `conn.http_version = Http2` (cleartext h2c —
				// the listener sets this when the peek prelude detects the
				// HTTP/2 connection preface). spec/crates/engine.md § _Dispatch table_.
				let prefer_h2 = alpn.as_deref() == Some(b"h2")
					|| matches!(conn.http_version.get(), Some(vane_core::HttpVersion::Http2));
				let result = if prefer_h2 {
					crate::upgrade::drive_h2_server(
						stream,
						Arc::clone(graph),
						*next,
						Arc::clone(conn),
						Arc::clone(&ctx.log),
						ctx.cancel.clone(),
						ctx.accept_cancel.clone(),
						ctx.verbosity,
					)
					.await
				} else {
					crate::upgrade::drive_h1_server(
						stream,
						Arc::clone(graph),
						*next,
						Arc::clone(conn),
						Arc::clone(&ctx.log),
						ctx.cancel.clone(),
						ctx.accept_cancel.clone(),
						ctx.verbosity,
					)
					.await
				};
				return match result {
					Ok(out) => {
						emit_trajectory(
							ctx,
							conn,
							&mut seq,
							TrajectoryOutcome::Terminated { node: cur, terminator: TerminatorOutcomeKind::Close },
						);
						Ok(out)
					}
					Err(e) => finish_error(ctx, conn, &mut seq, cur, e),
				};
			}

			Node::Terminate(tid) => {
				let term = sym[*tid];
				return match term {
					vane_core::Terminator::Close => {
						drop((l4.take(), req.take(), resp.take(), tunnel.take()));
						// Connection-level Terminate milestone (verbosity-
						// independent per spec/flow-model.md § _Flow log verbosity_).
						ctx.log.emit(FlowLogEvent {
							t: now_ms(),
							conn: conn.id,
							seq: bump(&mut seq),
							kind: FlowLogKind::Terminate,
							node: Some(cur),
							error: None,
							data: Some(serde_json::json!({
								"terminator": "close",
								"reason": "no matching rule",
							})),
						});
						emit_trajectory(
							ctx,
							conn,
							&mut seq,
							TrajectoryOutcome::Terminated { node: cur, terminator: TerminatorOutcomeKind::Close },
						);
						Ok(ExecutorOutput::Closed)
					}

					vane_core::Terminator::WriteHttpResponse => {
						let r = resp
							.take()
							.expect("phase invariant: WriteHttpResponse reached without a Response in scope");
						emit_trajectory(
							ctx,
							conn,
							&mut seq,
							TrajectoryOutcome::Terminated {
								node: cur,
								terminator: TerminatorOutcomeKind::WriteHttpResponse,
							},
						);
						Ok(ExecutorOutput::HttpResponse(r))
					}

					vane_core::Terminator::ByteTunnel => {
						drive_byte_tunnel(
							tunnel.take().expect("phase invariant: ByteTunnel reached without a Tunnel in scope"),
							&ctx.cancel,
						)
						.await;
						emit_trajectory(
							ctx,
							conn,
							&mut seq,
							TrajectoryOutcome::Terminated {
								node: cur,
								terminator: TerminatorOutcomeKind::ByteTunnel,
							},
						);
						Ok(ExecutorOutput::Tunneled)
					}
				};
			}
		}
	}
}

// Step recording
fn record_step(
	ctx: &mut FlowCtx,
	conn: &Arc<ConnContext>,
	seq: &mut u32,
	cur: NodeId,
	kind: FlowLogKind,
	branch: Option<bool>,
) {
	// spec/flow-model.md line 469: one `tracing::trace!` per iter, always on (gated
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

// ByteTunnel drive
async fn drive_byte_tunnel(t: Tunnel, cancel: &tokio_util::sync::CancellationToken) {
	match t {
		Tunnel::Bidi { mut client, mut upstream, mut close_reason_tx } => {
			// `copy_bidirectional` runs until both sides hit EOF or one errors.
			// `tokio::select!` adds a third axis: when `cancel` fires (listener
			// drain timeout, daemon shutdown), drop the copy future — the streams
			// are dropped along with it, the OS-level sockets close, and the peer
			// observes a reset. See spec/topology.md § _Listener lifecycle_ step 3.
			let reason = tokio::select! {
				biased;
				() = cancel.cancelled() => CloseReason::Cancelled,
				res = tokio::io::copy_bidirectional(&mut *client, &mut *upstream) => match res {
					Ok(_) => CloseReason::Graceful,
					Err(e) => CloseReason::ProtocolError(std::borrow::Cow::Owned(format!("byte tunnel io: {e}"))),
				},
			};

			if let Some(tx) = close_reason_tx.take() {
				// Receiver dropped is fine — Fetch may have moved on; the tunnel
				// io result is still observable in tracing if anyone wants it.
				let _ = tx.send(reason);
			}
		}
		Tunnel::Udp(mut udp) => {
			// The session forwarder runs in a task spawned by the fetch.
			// The executor's role here is to await the join future so the
			// caller observes session completion, and to surface
			// listener-level cancellation into the forwarder's own cancel
			// token. The join future's cleanup wrapper handles
			// dispatch-table removal as a side-effect.
			tokio::select! {
				biased;
				() = cancel.cancelled() => {
					udp.cancel.cancel();
					// Still await join so dispatch-table cleanup completes
					// before the executor tears down the connection context.
					let _ = (&mut udp.join).await;
				}
				_ = &mut udp.join => {}
			}
		}
	}
}

// Trajectory + error finalisation
fn finish_error(
	ctx: &mut FlowCtx,
	conn: &Arc<ConnContext>,
	seq: &mut u32,
	cur: NodeId,
	err: Error,
) -> Result<ExecutorOutput, Error> {
	metrics::counter!(
		"vane.errors.total",
		"kind" => err.kind_label(),
		"reason" => err.reason_label().unwrap_or("_"),
	)
	.increment(1);
	// Route through `SerializedError::from(&err)` so the capped-message
	// contract is uniform: `error = Some(SerializedError)` on every
	// emitted `FlowLogEvent` uses the same byte cap as the inline
	// trajectory message, and a verbose upstream error can no longer
	// blow up the sink-side memory budget.
	let message = vane_core::flow_log::TrajectoryErrorMessage::from(&err);
	emit_trajectory(ctx, conn, seq, TrajectoryOutcome::Error { node: cur, message });
	Err(err)
}

fn emit_trajectory(
	ctx: &mut FlowCtx,
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

fn emit_error_event(
	ctx: &mut FlowCtx,
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

// WASM middleware dispatch
const WASM_BODY_LIMIT_L7: usize = 1024 * 1024; // 1 MiB
const WASM_BODY_LIMIT_L4: usize = 64 * 1024; // 64 KiB

fn http_headers_to_wasm(map: &http::HeaderMap) -> Vec<Header> {
	map
		.iter()
		.filter_map(|(name, value)| {
			// `http::HeaderName::as_str` is documented to return the
			// canonical ASCII-lowercase form, so there is no need to
			// `to_ascii_lowercase()` the borrow before owning it. A
			// `debug_assert` keeps us honest if the `http` crate ever
			// regresses on that invariant.
			debug_assert!(
				name.as_str().bytes().all(|b| !b.is_ascii_uppercase()),
				"http::HeaderName::as_str() must return lower-case"
			);
			value.to_str().ok().map(|v| Header { name: name.as_str().to_owned(), value: v.to_owned() })
		})
		.collect()
}

fn wasm_headers_to_http(headers: Vec<Header>) -> http::HeaderMap {
	let mut map = http::HeaderMap::new();
	for h in headers {
		if let (Ok(name), Ok(value)) = (
			http::header::HeaderName::try_from(h.name.as_str()),
			http::header::HeaderValue::try_from(h.value.as_str()),
		) {
			map.insert(name, value);
		}
	}
	map
}

fn body_as_bytes_view(body: &Body, limit: usize) -> BytesView {
	match body {
		Body::Static(b) => {
			if b.len() > limit {
				BytesView { data: b[..limit].to_vec(), truncated: true }
			} else {
				BytesView { data: b.to_vec(), truncated: false }
			}
		}
		Body::Empty | Body::Stream(_) => BytesView { data: vec![], truncated: false },
	}
}

fn plugin_error_to_decision(pe: PluginError) -> Result<Decision, Error> {
	match pe {
		PluginError::Plugin { code, message, on_error_hint } => match on_error_hint.as_deref() {
			Some("force-close") => Ok(Decision::Short(ShortCircuit::Close(CloseReason::PolicyDenied(
				std::borrow::Cow::Owned(format!("plugin force-close: {code}: {message}")),
			)))),
			Some("internal") => {
				tracing::error!(plugin_code = %code, %message, "plugin returned internal error hint");
				Ok(Decision::Short(ShortCircuit::Close(CloseReason::PolicyDenied(
					std::borrow::Cow::Borrowed("plugin internal error"),
				))))
			}
			_ => Err(Error::middleware(format!("plugin error {code}: {message}"))),
		},
		PluginError::Trap(msg) => Err(Error::middleware(format!("plugin trap: {msg}"))),
		PluginError::Exhausted => Err(Error::middleware("plugin pool exhausted")),
	}
}

#[doc(hidden)]
pub async fn dispatch_wasm(
	w: &crate::flow_graph::WasmMiddleware,
	l4: &mut Option<L4Conn>,
	req: &mut Option<Request>,
	resp: &mut Option<Response>,
	conn: &Arc<ConnContext>,
) -> Result<Decision, Error> {
	let export = w.metadata.exports.iter().find(|e| e.name == w.export_name);
	let ctx: Vec<ContextEntry> = match export {
		Some(e) => crate::wasm_context::pack_context(&e.inspects, conn, w.module_id.0.as_ref()),
		None => Vec::new(),
	};
	match export.map(|e| e.kind) {
		Some(MiddlewareKind::L4Peek) => dispatch_wasm_l4_peek(w, ctx, conn).await,
		Some(MiddlewareKind::L4Bytes) => dispatch_wasm_l4_bytes(w, ctx, l4, conn).await,
		Some(MiddlewareKind::L7Request) => dispatch_wasm_l7_request(w, ctx, req).await,
		Some(MiddlewareKind::L7Response) => dispatch_wasm_l7_response(w, ctx, resp).await,
		None => Err(Error::middleware(format!(
			"export '{}' not found in plugin metadata for module '{}'",
			w.export_name, w.module_id.0,
		))),
	}
}

/// L4Peek dispatch: reads the listener-side peek snapshot off
/// `ConnContext.user` (spec/crates/engine.md § _Protocol detection_).
/// Reading from `L4Conn::Peeked` directly would consume the rewound
/// bytes and starve the L7 layer; the snapshot is the only safe source.
/// Cloning the `Bytes` is refcounted-cheap; releasing the lock before
/// `.await` keeps middleware free to take its own `conn.user` lock
/// without self-deadlock.
async fn dispatch_wasm_l4_peek(
	w: &crate::flow_graph::WasmMiddleware,
	ctx: Vec<ContextEntry>,
	conn: &Arc<ConnContext>,
) -> Result<Decision, Error> {
	let peek_buf: Option<bytes::Bytes> = {
		let user = conn.user.lock();
		user.get::<vane_core::PeekResult>().map(|r| r.buffer.clone())
	};
	// Absent `PeekResult` means the listener has no peek phase configured
	// (`needs_peek = false`); plugins legitimately see an empty slice.
	let peek = peek_buf.map(|b| b.to_vec()).unwrap_or_default();
	let input = L4PeekInput { peek, context: ctx };
	match w.runtime.invoke_l4_peek(&w.module_id, &w.export_name, &w.args_json, input).await {
		Ok(L4PeekDecision::Continue) => Ok(Decision::Continue),
		Ok(L4PeekDecision::Close) => Ok(Decision::Short(ShortCircuit::Close(
			CloseReason::PolicyDenied(std::borrow::Cow::Borrowed("plugin l4-peek close")),
		))),
		Err(pe) => plugin_error_to_decision(pe),
	}
}

/// L4Bytes dispatch: UDP reads the first cold-path datagram off
/// `UdpAssoc.first_packets`; TCP / TLS / Peeked reuse the L4Peek
/// snapshot off `ConnContext.user` because reading the rewound bytes
/// would starve the L7 layer (continued post-peek byte streaming for
/// L4Bytes is post-MVP). Each path truncates to `WASM_BODY_LIMIT_L4`.
async fn dispatch_wasm_l4_bytes(
	w: &crate::flow_graph::WasmMiddleware,
	ctx: Vec<ContextEntry>,
	l4: &mut Option<L4Conn>,
	conn: &Arc<ConnContext>,
) -> Result<Decision, Error> {
	let bytes_view = match l4.as_ref() {
		Some(L4Conn::Udp(assoc)) => {
			// Cold-path entry guarantees `first_packets` is non-empty.
			let pkt = assoc.first_packets.first().expect("UdpAssoc carries ≥1 datagram");
			if pkt.len() > WASM_BODY_LIMIT_L4 {
				BytesView { data: pkt[..WASM_BODY_LIMIT_L4].to_vec(), truncated: true }
			} else {
				BytesView { data: pkt.to_vec(), truncated: false }
			}
		}
		Some(L4Conn::Tcp(_) | L4Conn::Peeked(_) | L4Conn::Tls(_)) => {
			let peek_buf: Option<bytes::Bytes> = {
				let user = conn.user.lock();
				user.get::<vane_core::PeekResult>().map(|r| r.buffer.clone())
			};
			match peek_buf {
				Some(buf) if buf.len() > WASM_BODY_LIMIT_L4 => {
					BytesView { data: buf[..WASM_BODY_LIMIT_L4].to_vec(), truncated: true }
				}
				Some(buf) => BytesView { data: buf.to_vec(), truncated: false },
				None => BytesView { data: vec![], truncated: false },
			}
		}
		None => BytesView { data: vec![], truncated: false },
	};
	let input = L4BytesInput { bytes: bytes_view, context: ctx };
	match w.runtime.invoke_l4_bytes(&w.module_id, &w.export_name, &w.args_json, input).await {
		Ok(L4BytesDecision::Continue | L4BytesDecision::Tunnel) => Ok(Decision::Continue),
		Ok(L4BytesDecision::Close) => Ok(Decision::Short(ShortCircuit::Close(
			CloseReason::PolicyDenied(std::borrow::Cow::Borrowed("plugin l4-bytes close")),
		))),
		Err(pe) => plugin_error_to_decision(pe),
	}
}

/// L7Request dispatch: pack method / URI / headers + optional body
/// (`needs_body`-gated) into the WASM ABI; translate the plugin's
/// decision back to executor terms (Continue / Short(synth response) /
/// Close).
async fn dispatch_wasm_l7_request(
	w: &crate::flow_graph::WasmMiddleware,
	ctx: Vec<ContextEntry>,
	req: &mut Option<Request>,
) -> Result<Decision, Error> {
	let req_ref = req.as_mut().expect("phase invariant: L7Request wasm needs Request");
	let method = req_ref.method().to_string();
	let uri = req_ref.uri().to_string();
	let headers = http_headers_to_wasm(req_ref.headers());
	let body_view = if w.metadata.exports.iter().any(|e| e.name == w.export_name && e.needs_body) {
		Some(body_as_bytes_view(req_ref.body(), WASM_BODY_LIMIT_L7))
	} else {
		None
	};
	let input = L7RequestInput { method, uri, headers, body: body_view, context: ctx };
	match w.runtime.invoke_l7_request(&w.module_id, &w.export_name, &w.args_json, input).await {
		Ok(L7RequestDecision::Continue) => Ok(Decision::Continue),
		Ok(L7RequestDecision::Short(sr)) => {
			let response = synth_response_to_http(sr)?;
			Ok(Decision::Short(ShortCircuit::Response(response)))
		}
		Ok(L7RequestDecision::Close) => Ok(Decision::Short(ShortCircuit::Close(
			CloseReason::PolicyDenied(std::borrow::Cow::Borrowed("plugin l7-request close")),
		))),
		Err(pe) => plugin_error_to_decision(pe),
	}
}

/// L7Response dispatch: pack status / headers + optional body into
/// the WASM ABI; on `Modify` apply the plugin's overrides back to the
/// in-flight `Response` (status, headers, body).
async fn dispatch_wasm_l7_response(
	w: &crate::flow_graph::WasmMiddleware,
	ctx: Vec<ContextEntry>,
	resp: &mut Option<Response>,
) -> Result<Decision, Error> {
	let resp_ref = resp.as_mut().expect("phase invariant: L7Response wasm needs Response");
	let status = resp_ref.status().as_u16();
	let headers = http_headers_to_wasm(resp_ref.headers());
	let body_view = if w.metadata.exports.iter().any(|e| e.name == w.export_name && e.needs_body) {
		Some(body_as_bytes_view(resp_ref.body(), WASM_BODY_LIMIT_L7))
	} else {
		None
	};
	let input = L7ResponseInput { status, headers, body: body_view, context: ctx };
	match w.runtime.invoke_l7_response(&w.module_id, &w.export_name, &w.args_json, input).await {
		Ok(L7ResponseDecision::Continue) => Ok(Decision::Continue),
		Ok(L7ResponseDecision::Modify(mr)) => {
			if let Some(Ok(code)) = mr.status.map(http::StatusCode::try_from) {
				*resp_ref.status_mut() = code;
			}
			if let Some(hdrs) = mr.headers {
				*resp_ref.headers_mut() = wasm_headers_to_http(hdrs);
			}
			if let Some(body_bytes) = mr.body {
				*resp_ref.body_mut() = Body::Static(bytes::Bytes::from(body_bytes));
			}
			Ok(Decision::Continue)
		}
		Ok(L7ResponseDecision::Abort) => Ok(Decision::Short(ShortCircuit::Close(
			CloseReason::PolicyDenied(std::borrow::Cow::Borrowed("plugin l7-response abort")),
		))),
		Err(pe) => plugin_error_to_decision(pe),
	}
}

fn synth_response_to_http(sr: SynthResponse) -> Result<Response, Error> {
	let status = http::StatusCode::try_from(sr.status)
		.map_err(|_| Error::middleware(format!("plugin returned invalid status {}", sr.status)))?;
	let mut builder = http::Response::builder().status(status);
	for h in sr.headers {
		if let (Ok(name), Ok(value)) = (
			http::header::HeaderName::try_from(h.name.as_str()),
			http::header::HeaderValue::try_from(h.value.as_str()),
		) {
			builder = builder.header(name, value);
		}
	}
	builder
		.body(Body::Static(bytes::Bytes::from(sr.body)))
		.map_err(|e| Error::internal(format!("failed to build synth response: {e}")))
}

// Body collection for LazyBuffer middleware
enum CollectError {
	TooLarge,
	Io(Error),
}

async fn collect_body(body: &mut Body, limit: usize) -> Result<(), CollectError> {
	use http_body::Body as HttpBodyExt;
	let Body::Stream(s) = body else {
		return Ok(());
	};
	let mut collected = bytes::BytesMut::new();
	loop {
		// `s` is `Pin<Box<dyn HttpBody<...>>>`. Use `poll_fn` to drive the
		// async poll interface without requiring extra dependencies.
		use std::future::poll_fn;
		let frame_result = poll_fn(|cx| HttpBodyExt::poll_frame(s.as_mut(), cx)).await;
		match frame_result {
			None => break,
			Some(Err(e)) => return Err(CollectError::Io(e)),
			Some(Ok(frame)) => {
				if let Ok(data) = frame.into_data() {
					if collected.len() + data.len() > limit {
						return Err(CollectError::TooLarge);
					}
					collected.extend_from_slice(&data);
				}
			}
		}
	}
	*body = Body::Static(collected.freeze());
	Ok(())
}
