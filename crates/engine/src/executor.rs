use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use vane_core::{
	Body, BodySide, CloseReason, ConnContext, Decision, Error, FlowCtx, FlowLogEvent, FlowLogKind,
	FlowLogVerbosity, L4Conn, Node, NodeId, PredicateView, Request, Response, SerializedError,
	ShortCircuit, TerminatorOutcomeKind, TrajectoryOutcome, TrajectoryStep, Tunnel, UpstreamReason,
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
/// the `Response` onto a socket (hyper service-fn returns it from the H1/H2
/// handler; H3 is the same shape). 02-flow.md § _Execution model_'s
/// pseudocode currently shows `write_http_response(resp, conn, ctx).await`
/// as an internal helper — this design moves that write to the caller; see
/// the SPEC DEVIATION note in this chunk's report.
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
				// Hold the user-extension lock for exactly the lifetime of
				// the view: the peek slice borrows from the `PeekResult`
				// inside the guard, and the predicate `test` call needs to
				// read it before we release.
				let user = conn.user.lock();
				let peek = user.get::<vane_core::PeekResult>().map(|r| r.buffer.as_ref());
				let view = PredicateView::build(conn, req.as_ref(), l4.as_ref(), peek);
				let matched = sym[*predicate].test(&view);
				drop(user);
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Check, Some(matched));
				cur = if matched { *on_match } else { *on_miss };
			}

			Node::Middleware { id, next, on_error, .. } => {
				record_step(ctx, conn, &mut seq, cur, FlowLogKind::Middleware, None);
				let outcome = match &graph[*id] {
					MiddlewareInst::L4Peek(m) => {
						// PeekResult is keyed in `ConnContext.user` by the
						// listener-side prelude. Cloning the `Bytes` here is
						// cheap (refcounted) and lets us drop the user-
						// extension lock before the await — middleware bodies
						// are free to take their own `conn.user` locks
						// without deadlocking on themselves.
						let peek_buf: Option<bytes::Bytes> = {
							let user = conn.user.lock();
							user.get::<vane_core::PeekResult>().map(|r| r.buffer.clone())
						};
						if peek_buf.is_none() {
							tracing::warn!(
								conn_id = %conn.id,
								"L4Peek dispatched without PeekResult — listener prelude must run first",
							);
						}
						let peek_slice: &[u8] = peek_buf.as_deref().unwrap_or(&[]);
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
				};

				match outcome {
					Ok(Decision::Continue) => cur = *next,
					Ok(Decision::Short(ShortCircuit::Response(r))) => {
						// 02-flow.md § _Execution model_: an L7 request
						// middleware that returns `Short(Response)` parks
						// the response in `resp` and jumps to the
						// listener-level synth `Terminate(WriteHttpResponse)`
						// installed by the lower pass (see § _`FlowGraph`
						// metadata_). The `request` slot is dropped because
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
						// (see `02-flow.md` § _`Terminator::Close` at L4 vs
						// inside an HTTP server_); the L4 listener drops the
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
				// from `*next`. See 02-flow.md § _Execution model_ (Upgrade arm).
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
				// HTTP/2 connection preface). 06-l4.md § _Dispatch decision
				// table_.
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
						// independent per 02-flow.md § _Flow log verbosity_).
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

// --- Step recording -----------------------------------------------------

fn record_step(
	ctx: &mut FlowCtx,
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

// --- ByteTunnel drive ---------------------------------------------------

async fn drive_byte_tunnel(t: Tunnel, cancel: &tokio_util::sync::CancellationToken) {
	match t {
		Tunnel::Bidi { mut client, mut upstream, mut close_reason_tx } => {
			// `copy_bidirectional` runs until both sides hit EOF or one errors.
			// `tokio::select!` adds a third axis: when `cancel` fires (listener
			// drain timeout, daemon shutdown), drop the copy future — the streams
			// are dropped along with it, the OS-level sockets close, and the peer
			// observes a reset. See 01-topology.md § _Listener lifecycle_ step 3.
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

// --- Trajectory + error finalisation -----------------------------------

fn finish_error(
	ctx: &mut FlowCtx,
	conn: &Arc<ConnContext>,
	seq: &mut u32,
	cur: NodeId,
	err: Error,
) -> Result<ExecutorOutput, Error> {
	let message = std::borrow::Cow::Owned(err.to_string());
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

// --- Body collection for LazyBuffer middleware ----------------------------

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
