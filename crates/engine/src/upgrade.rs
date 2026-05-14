//! `Node::Upgrade` execution — L4 → L7 bridge. Hands a byte stream
//! (plain TCP or TLS-terminated) to a hyper H1 or H2 server builder;
//! each decoded `Request` walks the L7 sub-graph from the
//! `Upgrade.next` node.
//!
//! See `spec/crates/engine-tls.md` § _Termination flow (L4 → L7 upgrade)_,
//! `spec/flow-model.md` § _Executor_ (Upgrade arm),
//! `spec/crates/engine.md` (H1 / H2 paths).
//!
//! Out of MVP scope (separately tracked): H3, WS-over-h2 (RFC 8441).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite, copy_bidirectional};
use tokio_util::sync::CancellationToken;
use vane_core::{
	Body, ConnContext, Error, FlowCtx, FlowLogSink, FlowLogVerbosity, HttpVersion, NodeId, Request,
	Response, TrajectoryBuilder,
};

use crate::body_adapter::IncomingAdapter;
use crate::executor::{ExecutorInput, ExecutorOutput, execute};
use crate::fetch::websocket_upgrade::StashedUpstreamUpgrade;
use crate::flow_graph::FlowGraph;
use crate::time::now_unix_ms;

/// Stash for the `JoinHandle` of a post-101 WebSocket byte-tunnel.
/// The H1 service-fn spawns the tunnel asynchronously so hyper can
/// write the 101 to the client before the upgrade hands off the
/// socket; the outer [`drive_h1_server`] then takes the handle and
/// awaits it so the listener's in-flight JoinSet sees the connection
/// alive for the full tunnel lifetime (otherwise the H1 connection
/// task ends at the 101 hand-off and the WS tunnel runs detached,
/// invisible to daemon shutdown / listener reconcile drain logic).
///
/// `Arc<parking_lot::Mutex<Option<JoinHandle>>>` so the stash is
/// `Clone` (cheap refcount) and the take-once semantics survive a
/// concurrent re-entry (though only one re-entry should ever see
/// `Some`).
#[derive(Clone)]
pub(crate) struct PendingUpgradeTask(
	pub(crate) Arc<parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>>,
);

impl PendingUpgradeTask {
	fn new(handle: tokio::task::JoinHandle<()>) -> Self {
		Self(Arc::new(parking_lot::Mutex::new(Some(handle))))
	}

	pub(crate) fn take(&self) -> Option<tokio::task::JoinHandle<()>> {
		self.0.lock().take()
	}
}

/// Drive a byte stream as an H1 server. For each decoded request, build a
/// fresh `FlowCtx` (sharing `log` / `cancel` / `verbosity` from the outer
/// L4 ctx, with its own `TrajectoryBuilder`) and call the executor with
/// the L7 sub-graph entry. The executor's `ExecutorOutput::HttpResponse`
/// flows back to the service-fn, which returns it to hyper for wire
/// serialisation.
///
/// `S` is generic so a plain `TcpStream` (cleartext listener) and a
/// `tokio_rustls::server::TlsStream<TcpStream>` (TLS-terminated listener)
/// can both feed the same H1 driver — the only difference is what the
/// listener loop hands us.
///
/// `Ok(ExecutorOutput::Closed)` is returned when the H1 connection ends —
/// either the client EOF'd, or `Connection: close` closed the last
/// request. The outer L4 `execute` simply propagates this back.
///
/// # Errors
/// Surfaces as `Error::protocol("h1 serve_connection").with_source(...)`
/// any hyper-level connection failure (malformed framing, premature EOF
/// during a request, etc.). Per-request executor errors are translated
/// to a synthetic 500 *inside* the service-fn so the connection itself
/// can stay alive for the next request on a keep-alive socket.
#[allow(
	clippy::too_many_lines,
	reason = "the body length is dominated by a single hyper service_fn closure that captures graph + conn + log + cancel + verbosity + l7_entry + h1_max_header_bytes. Extracting the closure body would just move those captures into a ctx struct without compressing the actual logic"
)]
#[allow(
	clippy::too_many_arguments,
	reason = "h1 driver wiring: graph/entry/conn/log/cancel/accept_cancel/verbosity each thread one piece of per-conn state; bag struct would just rename the noise"
)]
pub(crate) async fn drive_h1_server<S>(
	stream: S,
	graph: Arc<FlowGraph>,
	l7_entry: NodeId,
	conn: Arc<ConnContext>,
	log: Arc<dyn FlowLogSink>,
	cancel: CancellationToken,
	accept_cancel: CancellationToken,
	verbosity: FlowLogVerbosity,
) -> Result<ExecutorOutput, Error>
where
	S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
	// Record negotiated HTTP version once on the shared ConnContext so L7
	// predicates / middleware can read it. H1 only this round.
	let _ = conn.http_version.set(HttpVersion::Http1_1);

	// Extract L1 security limits before moving `graph` into the closure.
	let h1_max_buf = graph.security_cfg().max_header_bytes.saturating_mul(4).max(8_192);
	let h1_max_headers = graph.security_cfg().max_headers_count;
	let h1_header_timeout = graph.security_cfg().header_timeout;
	let h1_max_header_bytes = graph.security_cfg().max_header_bytes;

	// Outer cancel handle drives the connection-level select below.
	// `svc` keeps its own clones for per-request executor wiring.
	let conn_cancel = cancel.clone();
	let conn_accept_cancel = accept_cancel.clone();
	// The post-`select!` re-join of any stashed WS-tunnel JoinHandle
	// reads `conn.user`, so we keep a clone outside the service-fn's
	// move-into-closure capture. The service-fn itself clones from
	// the inner shadow `conn` per request.
	let conn_outer = Arc::clone(&conn);
	let svc = service_fn(move |mut req: hyper::Request<Incoming>| {
		let graph = Arc::clone(&graph);
		let conn = Arc::clone(&conn);
		let log = Arc::clone(&log);
		let cancel = cancel.clone();
		let accept_cancel = accept_cancel.clone();
		async move {
			// Precise header byte check (name + ": " + value + "\r\n" = +4).
			// hyper's `max_buf_size` provides a coarse upper bound on the
			// read buffer; this check enforces the spec limit precisely on
			// parsed header fields.
			let header_bytes: usize =
				req.headers().iter().map(|(name, value)| name.as_str().len() + value.len() + 4).sum();
			if header_bytes > h1_max_header_bytes {
				return Ok::<Response, std::convert::Infallible>(
					http::Response::builder()
						.status(431)
						.header("connection", "close")
						.body(Body::Empty)
						.expect("static 431"),
				);
			}

			// Pull the client-side `OnUpgrade` future out of the
			// request's extensions BEFORE we adapt the body. Hyper's
			// upgrade machinery sets this when it parses the
			// `Upgrade: websocket` request header. We hold it on the
			// stack across `execute(...)` so that, when an upstream
			// `WebSocketUpgrade` fetch produces a 101, we can `await`
			// the client upgrade and bridge the two ends with
			// `copy_bidirectional`. For non-WS requests this is a
			// dropped-future no-op.
			let client_on_upgrade = hyper::upgrade::on(&mut req);

			let vane_req: Request =
				req.map(|incoming| Body::Stream(Box::pin(IncomingAdapter::new(incoming))));

			// URI path is intentionally absent from this INFO span:
			// path commonly carries tokens (verify / reset / OAuth
			// state), user / tenant / order IDs, and other PII that
			// `tracing-broadcast` would fan into the `tail_log`
			// mgmt stream where every operator with mgmt access can
			// read it. The flow log already captures full URI on
			// per-rule opt-in (`spec/flow-model.md`
			// § _Flow log verbosity_), so debugging by path goes
			// through that channel instead. Mirrors the H2 / H3
			// spans further down.
			let span = tracing::info_span!(
				"request",
				conn = %conn.id,
				method = %vane_req.method(),
			);

			// Keep a separate clone for the post-101 WS-tunnel spawn
			// below — `FlowCtx::cancel` moves the original into the
			// executor's per-request state.
			let tunnel_cancel = cancel.clone();
			let mut ctx = FlowCtx {
				span,
				log,
				cancel,
				accept_cancel,
				verbosity,
				trajectory: TrajectoryBuilder::new(conn.id, l7_entry, now_unix_ms()),
			};

			let result =
				execute(&graph, l7_entry, ExecutorInput::L7(Box::new(vane_req)), &conn, &mut ctx).await;

			match result {
				Ok(ExecutorOutput::HttpResponse(r))
					if r.status() == http::StatusCode::SWITCHING_PROTOCOLS =>
				{
					// `WebSocketUpgrade` fetch path: stash → service-fn
					// rendezvous. The fetch put the upgraded upstream IO
					// on `conn.user` before returning the 101; we take
					// it here and spawn a `copy_bidirectional` task that
					// bridges client ↔ upstream once the client upgrade
					// completes. Bytes flow opaquely; vane never decodes
					// WebSocket frames.
					let stashed = conn
						.user
						.lock()
						.get::<StashedUpstreamUpgrade>()
						.cloned()
						.and_then(|s| s.take().map(|io| (s, io)));
					let Some(stashed) = stashed else {
						tracing::error!(
							conn_id = %conn.id,
							"101 returned without stashed upstream IO — ws fetch invariant violated; synthesising 502",
						);
						return Ok::<Response, std::convert::Infallible>(
							http::Response::builder().status(502).body(Body::Empty).expect("static"),
						);
					};
					let (_holder, mut upstream_io) = stashed;
					let conn_id = conn.id;
					// `tunnel_cancel` is a clone of the per-request
					// `force_cancel` token (the executor's
					// `FlowCtx::cancel` got the original). Inside the
					// spawned task's `tokio::select!` it lets the
					// daemon-wide drain budget actually evict an
					// in-flight WS tunnel — without it,
					// `copy_bidirectional` would run until one end's
					// IO closes, ignoring the listener's shutdown
					// signal entirely.
					let tunnel_handle = tokio::spawn(async move {
						let upgraded = match client_on_upgrade.await {
							Ok(u) => u,
							Err(e) => {
								tracing::warn!(?e, %conn_id, "client ws upgrade await failed");
								return;
							}
						};
						let mut client_io = TokioIo::new(upgraded);
						tokio::select! {
							biased;
							() = tunnel_cancel.cancelled() => {
								tracing::debug!(%conn_id, "ws byte tunnel cancelled by force_cancel");
							}
							r = copy_bidirectional(&mut client_io, &mut *upstream_io) => {
								if let Err(e) = r {
									tracing::debug!(?e, %conn_id, "ws byte tunnel ended with io error");
								}
							}
						}
					});
					// Stash the JoinHandle so `drive_h1_server` can
					// await it after `server_conn` returns. This keeps
					// the H1 connection's listener-side JoinSet task
					// alive for the full WS-tunnel lifetime, which is
					// what makes `force_cancel` + drain accounting
					// observe the tunnel rather than skip past it.
					conn.user.lock().insert(PendingUpgradeTask::new(tunnel_handle));
					Ok(r)
				}
				Ok(ExecutorOutput::HttpResponse(r)) => Ok::<Response, std::convert::Infallible>(r),
				Ok(ExecutorOutput::Closed) => {
					// L7 path ended via Terminate(Close) without producing a
					// Response. The L4 analogue is TCP RST; the L7 analogue
					// inside hyper is "synthesise a status that signals
					// proxy-layer no-route, then close the H1 connection so
					// the next request on the same socket doesn't see a
					// stale rule-set":
					//
					//   - 404 for HTTP/1.x clients (broadest compatibility;
					//     RFC 9110 § 15.5.5 — origin sense is technically
					//     wrong but H1 clients react sanely).
					//   - 421 Misdirected Request for HTTP/2 / HTTP/3 (RFC
					//     9110 § 15.5.20 — "the server is not configured to
					//     produce responses for this URI"; semantically the
					//     accurate match for proxy-layer no-route).
					//
					// We're inside `drive_h1_server`, so `conn.http_version`
					// is always `Http1_1` here — but reading the OnceLock
					// keeps the choice future-proof when H2 / H3 driver
					// siblings land.
					let status = match conn.http_version.get() {
						Some(HttpVersion::Http2 | HttpVersion::Http3) => 421,
						_ => 404,
					};
					Ok(
						http::Response::builder()
							.status(status)
							.header("connection", "close")
							.body(Body::Empty)
							.expect("static"),
					)
				}
				Ok(ExecutorOutput::Tunneled) => {
					// WS-101 lands here; not in MVP scope. Surface a 500 so
					// the client doesn't hang.
					tracing::warn!("L7 tunnel terminator (WebSocket) not yet supported — synthesising 500",);
					Ok(http::Response::builder().status(500).body(Body::Empty).expect("static"))
				}
				Err(e) => {
					tracing::warn!(error = %e, "L7 execute returned Err — synthesising 500");
					Ok(http::Response::builder().status(500).body(Body::Empty).expect("static"))
				}
			}
		}
	});

	let io = TokioIo::new(stream);
	// `.with_upgrades()` keeps the upgrade channel alive past the 101
	// response; without it, the server-side `OnUpgrade` future the
	// service-fn captured would close immediately and the
	// `copy_bidirectional` spawn would never see the upgraded IO.
	//
	// L1 builder knobs:
	// - `max_buf_size`: coarse IO buffer cap (4× max_header_bytes);
	//   limits raw bytes hyper buffers before the parse.
	// - `max_headers`: precise header-count cap; hyper returns 431 if
	//   exceeded before our service-fn runs.
	// - `header_read_timeout` + `TokioTimer`: per-request header
	//   completion deadline starting from the first byte received
	//   (covers keep-alive idle requests after the first). The L4
	//   peek phase covers the very first bytes with the same duration.
	let server_conn = {
		let mut b = hyper::server::conn::http1::Builder::new();
		b.max_buf_size(h1_max_buf)
			.max_headers(h1_max_headers)
			.timer(TokioTimer::new())
			.header_read_timeout(h1_header_timeout);
		b.serve_connection(io, svc).with_upgrades()
	};
	tokio::pin!(server_conn);

	// Watch both cancel tokens alongside the hyper connection. A
	// keep-alive idle H1 connection has no server-side IO to drive
	// `serve_connection` toward EOF, so a cancel signal is the only
	// way to pull a well-behaved client off our process during
	// shutdown.
	//
	// `accept_cancel` (soft drain): the listener no longer accepts
	// new connections — tell hyper to stop reusing this one. We call
	// `graceful_shutdown` (sends `Connection: close` on the next
	// response and finishes any in-flight request, then closes the
	// socket) and keep awaiting so the in-flight request — if any —
	// gets to write its body. This is what lets idle keep-alive
	// clients exit immediately rather than camping until the drain
	// budget runs out.
	//
	// `conn_cancel` (force_cancel): we've passed the soft-drain
	// window. Trigger graceful_shutdown the same way and re-await
	// once; any still-pending request finishes against the (now
	// hard-cancelled) FlowCtx::cancel and the socket closes.
	//
	// Any post-upgrade WebSocket byte tunnel runs in its own spawned
	// task that observes `FlowCtx::cancel` independently, so neither
	// graceful_shutdown yanks the upgraded socket out from under it.
	let outcome = tokio::select! {
		biased;
		result = server_conn.as_mut() => result,
		() = conn_accept_cancel.cancelled() => {
			server_conn.as_mut().graceful_shutdown();
			server_conn.as_mut().await
		}
		() = conn_cancel.cancelled() => {
			server_conn.as_mut().graceful_shutdown();
			server_conn.as_mut().await
		}
	};
	outcome.map_err(|e| Error::protocol("h1 serve_connection").with_source(e))?;

	// If the per-request service-fn handed off a WebSocket upgrade
	// (status 101 / `Connection: upgrade`), the byte-tunnel runs in a
	// `tokio::spawn`'d task whose handle was stashed on
	// `conn.user`. The hyper `serve_connection().with_upgrades()`
	// future has already completed (the upgrade-machinery delivered
	// the Upgraded handle); without this re-join the H1 connection's
	// listener-side JoinSet task would end here and the WS tunnel
	// would run detached. Awaiting the handle keeps the listener's
	// shutdown drain accounting accurate and gives `force_cancel`
	// (the same token plumbed into the spawn's `select!`) an
	// observable end-of-life. JoinError from a panicked task is
	// logged but does not propagate — the H1 connection has already
	// closed.
	let pending = conn_outer.user.lock().get::<PendingUpgradeTask>().cloned().and_then(|p| p.take());
	if let Some(handle) = pending
		&& let Err(e) = handle.await
	{
		tracing::debug!(error = ?e, conn_id = %conn_outer.id, "ws byte tunnel task ended abnormally");
	}

	Ok(ExecutorOutput::Closed)
}

/// Drive a byte stream as an H2 server. Same per-request executor
/// re-entry pattern as [`drive_h1_server`]; differences:
///
/// 1. Driven by `hyper::server::conn::http2::Builder` with a
///    `hyper_util::rt::TokioExecutor` for stream-task spawning.
/// 2. No `OnUpgrade` dance — h2 has no 101 status; WS-over-h2
///    (RFC 8441) is out of this round's scope, so an executor that
///    returns a 101 here gets translated to a 500 (h2 clients never
///    expect 101).
/// 3. Closed → 421 Misdirected Request (RFC 9110 § 15.5.20). h2
///    clients understand this as "this server isn't authoritative for
///    this URI"; the H1 driver picks 404 instead per its own contract.
///
/// `S` is generic on the same trait set as the H1 driver so a
/// `tokio_rustls::server::TlsStream<TcpStream>` (the only path that
/// reaches H2 today, since H2 cleartext requires explicit prior
/// knowledge or a 101 upgrade and we don't advertise it) can drive
/// this directly.
///
/// # Errors
/// Surfaces hyper-level connection failures as
/// `Error::protocol("h2 serve_connection").with_source(...)`.
/// Per-request executor errors are translated to a synthetic 500
/// inside the service-fn so the connection itself stays alive.
#[allow(
	clippy::too_many_arguments,
	reason = "h2 driver wiring: same shape as h1; bag struct would just rename the noise"
)]
pub(crate) fn drive_h2_server<S>(
	stream: S,
	graph: Arc<FlowGraph>,
	l7_entry: NodeId,
	conn: Arc<ConnContext>,
	log: Arc<dyn FlowLogSink>,
	cancel: CancellationToken,
	accept_cancel: CancellationToken,
	verbosity: FlowLogVerbosity,
) -> Pin<Box<dyn Future<Output = Result<ExecutorOutput, Error>> + Send + 'static>>
where
	S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
	// Returning `Pin<Box<dyn Future + Send>>` (rather than `async fn`)
	// breaks an infinite-`Send`-bounded type that arises from the
	// `execute → drive_h2_server → service_fn → execute` cycle. Hyper's
	// h2 builder requires the service-fn future to be `Send`, which
	// recursively forces `execute`'s future to be `Send`, which contains
	// `drive_h2_server`'s future via the `Node::Upgrade` arm. With an
	// opaque async-fn return, the compiler cannot prove this; with a
	// boxed dyn-future, the recursion goes through a sized erased type
	// and resolves cleanly.
	Box::pin(async move {
		// The listener has usually already populated this from the
		// negotiated ALPN; a redundant set is a silent no-op (`OnceLock`).
		let _ = conn.http_version.set(HttpVersion::Http2);

		// Outer cancel handles drive the connection-level select below.
		// `svc` keeps its own clones for per-request executor wiring.
		let conn_cancel = cancel.clone();
		let conn_accept_cancel = accept_cancel.clone();
		let svc = service_fn(move |req: hyper::Request<Incoming>| {
			let graph = Arc::clone(&graph);
			let conn = Arc::clone(&conn);
			let log = Arc::clone(&log);
			let cancel = cancel.clone();
			let accept_cancel = accept_cancel.clone();
			async move {
				let vane_req: Request =
					req.map(|incoming| Body::Stream(Box::pin(IncomingAdapter::new(incoming))));

				// `path` intentionally absent — see the H1 driver's
				// span comment above for the PII rationale.
				let span = tracing::info_span!(
					"request",
					conn = %conn.id,
					version = "h2",
					method = %vane_req.method(),
				);

				let mut ctx = FlowCtx {
					span,
					log,
					cancel,
					accept_cancel,
					verbosity,
					trajectory: TrajectoryBuilder::new(conn.id, l7_entry, now_unix_ms()),
				};

				let result =
					execute(&graph, l7_entry, ExecutorInput::L7(Box::new(vane_req)), &conn, &mut ctx).await;

				match result {
					Ok(ExecutorOutput::HttpResponse(r))
						if r.status() == http::StatusCode::SWITCHING_PROTOCOLS =>
					{
						// 101 over h2 is a protocol violation — h2 clients
						// never expect the Upgrade handshake. WS-over-h2
						// (RFC 8441) is a separate codepath we don't yet
						// implement; surface a 500 so the client gets a
						// pointed signal instead of a malformed response.
						tracing::warn!(
							conn_id = %conn.id,
							"h2 service-fn received 101 from executor; synthesising 500 (WS-over-h2 unsupported)",
						);
						Ok::<Response, std::convert::Infallible>(
							http::Response::builder().status(500).body(Body::Empty).expect("static"),
						)
					}
					Ok(ExecutorOutput::HttpResponse(r)) => Ok::<Response, std::convert::Infallible>(r),
					Ok(ExecutorOutput::Closed) => {
						// L7 no-route in h2 land — 421 Misdirected Request
						// is the semantically accurate match (RFC 9110
						// § 15.5.20). Mirrors the H1 driver's 404, but
						// uses h2-native semantics so clients can retry
						// against a different authority.
						Ok(http::Response::builder().status(421).body(Body::Empty).expect("static"))
					}
					Ok(ExecutorOutput::Tunneled) => {
						tracing::warn!("L7 tunnel terminator over h2 unsupported — synthesising 500");
						Ok(http::Response::builder().status(500).body(Body::Empty).expect("static"))
					}
					Err(e) => {
						tracing::warn!(error = %e, "L7 execute returned Err — synthesising 500");
						Ok(http::Response::builder().status(500).body(Body::Empty).expect("static"))
					}
				}
			}
		});

		let io = TokioIo::new(stream);
		let server_conn =
			hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
				.serve_connection(io, svc);
		tokio::pin!(server_conn);

		// H2 graceful_shutdown sends `GOAWAY` and waits for in-flight
		// streams to finish; idle multiplexed connections then exit
		// without further client traffic. Wire both cancel tiers in
		// (same logic as the H1 driver):
		//
		// - `accept_cancel` (soft drain): emit `GOAWAY` as soon as the
		//   listener stops accepting — idle multiplexed clients see
		//   the connection exit immediately instead of camping until
		//   the drain budget runs out.
		// - `conn_cancel` (force_cancel): final hammer; re-await once
		//   so any still-in-flight stream finishes against the
		//   hard-cancelled FlowCtx.
		let outcome = tokio::select! {
			biased;
			result = server_conn.as_mut() => result,
			() = conn_accept_cancel.cancelled() => {
				server_conn.as_mut().graceful_shutdown();
				server_conn.as_mut().await
			}
			() = conn_cancel.cancelled() => {
				server_conn.as_mut().graceful_shutdown();
				server_conn.as_mut().await
			}
		};
		outcome.map_err(|e| Error::protocol("h2 serve_connection").with_source(e))?;

		Ok(ExecutorOutput::Closed)
	})
}

/// Drive an H3 connection. Mirrors [`drive_h1_server`] / [`drive_h2_server`]
/// in shape: builds a fresh `FlowCtx` per request, hands the
/// `http::Request<Body>` to the executor, then writes the resulting
/// response back through the h3 stream's `send_response` /
/// `send_data` / `send_trailers` / `finish`. Stream-level errors close
/// the stream; connection-level errors return.
///
/// This driver is reachable only via the H3 listener path, which
/// pre-populates `ConnContext.transport = Udp` and `http_version =
/// Http3`.
#[cfg(feature = "h3")]
pub(crate) async fn drive_h3_server(
	listener_addr: std::net::SocketAddr,
	quic_conn: quinn::Connection,
	graph: Arc<arc_swap::ArcSwap<FlowGraph>>,
	log: Arc<dyn FlowLogSink>,
	accept_cancel: CancellationToken,
	force_cancel: CancellationToken,
	verbosity: Arc<crate::verbosity::VerbosityState>,
) {
	// Alias for the existing per-stream FlowCtx wiring below — every
	// stream still binds its `FlowCtx::cancel` to `force_cancel`, so
	// nothing about the stream-level lifecycle changed. `accept_cancel`
	// only governs whether the H3 connection-level accept loop below
	// pulls a fresh stream out of the QUIC connection.
	let cancel = force_cancel.clone();
	let remote = quic_conn.remote_address();
	let conn_id = crate::listener::next_conn_id();
	let conn = Arc::new(vane_core::ConnContext::new(
		conn_id,
		remote,
		listener_addr,
		vane_core::Transport::Udp,
		std::time::Instant::now(),
	));
	*conn.tls.lock() = Some(vane_core::TlsInfo {
		alpn: Some(std::sync::Arc::from(&b"h3"[..])),
		..vane_core::TlsInfo::default()
	});
	let _ = conn.http_version.set(HttpVersion::Http3);

	let h3_quic_conn = h3_quinn::Connection::new(quic_conn);
	let mut h3_conn = match h3::server::Connection::new(h3_quic_conn).await {
		Ok(c) => c,
		Err(e) => {
			tracing::debug!(error = %e, conn_id = %conn.id, "h3 server::Connection setup failed");
			return;
		}
	};

	loop {
		tokio::select! {
			biased;
			() = force_cancel.cancelled() => return,
			() = accept_cancel.cancelled() => {
				// Soft drain: stop accepting new H3 streams on this
				// connection, but let any in-flight stream complete.
				// Each handle_h3_request task ran to completion
				// already-spawned via `tokio::spawn`; we exit the
				// outer accept loop and the connection enters QUIC's
				// natural drain.
				tracing::debug!(conn_id = %conn.id, "h3 driver received accept_cancel; stopping stream accept");
				return;
			}
			accepted = h3_conn.accept() => {
				match accepted {
					Ok(Some(resolver)) => {
						let (req, stream) = match resolver.resolve_request().await {
							Ok(t) => t,
							Err(e) => {
								tracing::debug!(error = %e, conn_id = %conn.id, "h3 resolve_request failed");
								continue;
							}
						};
						let graph_snap = graph.load_full();
						let Some(listener_entry) =
							graph_snap.symbolic().entries.get(&listener_addr).copied()
						else {
							tracing::debug!(
								?listener_addr,
								conn_id = %conn.id,
								"h3 stream: no entry in active graph; dropping",
							);
							continue;
						};
						// Peel the listener entry's `Node::Upgrade` to land on the
						// L7 sub-graph. The TCP path does this inside the executor's
						// `Upgrade` arm by passing `*next` to `drive_h1_server` /
						// `drive_h2_server`; H3 has no L4 phase (quinn owns the
						// QUIC handshake), so the H3 driver enters the executor
						// at L7 directly. Dropping the connection cleanly when the
						// entry isn't `Upgrade` matches the executor's phase
						// invariant — non-`Upgrade` entries on `Http` listeners
						// would not be reachable through the L4→L7 path either.
						let entry = if let Some(vane_core::Node::Upgrade { next }) =
							graph_snap.symbolic().nodes.get(listener_entry.get() as usize)
						{
							*next
						} else {
							tracing::debug!(
								?listener_addr,
								conn_id = %conn.id,
								"h3 stream: listener entry is not Node::Upgrade; dropping",
							);
							continue;
						};
						let sctx = H3StreamCtx {
							graph: graph_snap,
							entry,
							conn: Arc::clone(&conn),
							log: Arc::clone(&log),
							cancel: cancel.clone(),
							accept_cancel: accept_cancel.clone(),
							verbosity: verbosity.current(),
						};
						tokio::spawn(handle_h3_request(req, stream, sctx));
					}
					Ok(None) => return,
					Err(e) => {
						tracing::debug!(error = %e, conn_id = %conn.id, "h3 accept ended with err");
						return;
					}
				}
			}
		}
	}
}

/// Per-stream executor inputs that are constant across the H3
/// request's lifetime: the captured graph snapshot, the resolved L7
/// entry, the shared `ConnContext`, and the FlowCtx primitives (log
/// sink, cancel token, verbosity snapshot). One instance is built per
/// accepted bidi stream and moved into [`handle_h3_request`].
#[cfg(feature = "h3")]
pub(crate) struct H3StreamCtx {
	pub graph: Arc<FlowGraph>,
	pub entry: NodeId,
	pub conn: Arc<vane_core::ConnContext>,
	pub log: Arc<dyn FlowLogSink>,
	pub cancel: CancellationToken,
	pub accept_cancel: CancellationToken,
	pub verbosity: vane_core::FlowLogVerbosity,
}

/// Per-stream handler — runs the executor then writes the response
/// back through the h3 stream half. Body frames are pulled via
/// `http_body::Body::poll_frame` and forwarded to `send_data` /
/// `send_trailers`; the stream is `finish`ed at end.
///
/// The bidi stream is split before invoking the executor: the recv
/// half is wrapped in `H3Body` and handed to the executor as
/// `Body::Stream(...)` so middleware / fetch can read request frames
/// as they arrive, while the send half is held back for response
/// writeback. The `H3Body` pump task (spawned inside `H3Body::new`)
/// drives `recv_data` in parallel with the executor's read of the
/// body channel, so a streaming upstream sees bytes as the client
/// sends them rather than after a full request-body buffer.
#[cfg(feature = "h3")]
async fn handle_h3_request(
	req: http::Request<()>,
	stream: h3::server::RequestStream<h3_quinn::BidiStream<bytes::Bytes>, bytes::Bytes>,
	sctx: H3StreamCtx,
) {
	use http_body::Body as _;
	let H3StreamCtx { graph, entry, conn, log, cancel, accept_cancel, verbosity } = sctx;
	let (mut parts, _empty) = req.into_parts();

	// `h3` sets `parts.version = HTTP/3.0`. The L7 executor + middleware
	// path is version-agnostic (predicates read `conn.http_version`,
	// not `req.version()`); but `hyper_util::Client::request` — which
	// `HttpProxyFetch` dispatches through — only matches HTTP/1.x and
	// HTTP/2.0 and rejects HTTP/3.0 with `UserUnsupportedVersion`.
	// Normalise to HTTP/1.1 so cross-version bridging (H3 client → H1 /
	// H2 upstream) actually works. The wire-level version on the H3
	// listener side is preserved in `conn.http_version = Http3`, set
	// at connection accept above.
	parts.version = http::Version::HTTP_11;

	// Split the bidi stream so the request body can stream concurrently
	// with response writeback. h3's `RequestStream::split` returns
	// `(send, recv)` — the recv half feeds `H3Body` (which spawns its
	// own pump task pulling `recv_data` into a bounded channel), the
	// send half is held for `send_response` / `send_data` /
	// `send_trailers` / `finish` after the executor returns.
	let (mut send_stream, recv_stream) = stream.split();
	let body =
		Body::from_producer(h3_body::H3Body::new(h3_body::ServerStreamSource::new(recv_stream)));
	let vane_req: Request = http::Request::from_parts(parts, body);

	// `path` intentionally absent — see the H1 driver's span comment
	// above for the PII rationale.
	let span = tracing::info_span!(
		"request",
		conn = %conn.id,
		method = %vane_req.method(),
	);
	let mut ctx = FlowCtx {
		span,
		log,
		cancel,
		accept_cancel,
		verbosity,
		trajectory: TrajectoryBuilder::new(conn.id, entry, now_unix_ms()),
	};

	let exec_out =
		execute(&graph, entry, ExecutorInput::L7(Box::new(vane_req)), &conn, &mut ctx).await;

	let response = match exec_out {
		Ok(ExecutorOutput::HttpResponse(r)) => r,
		Ok(ExecutorOutput::Closed) => {
			http::Response::builder().status(421).body(Body::Empty).expect("static 421")
		}
		Ok(ExecutorOutput::Tunneled) => {
			tracing::warn!("L7 tunnel terminator (WebSocket) not supported on H3 — synthesising 500");
			http::Response::builder().status(500).body(Body::Empty).expect("static 500")
		}
		Err(e) => {
			tracing::warn!(error = %e, "L7 execute returned Err — synthesising 500");
			http::Response::builder().status(500).body(Body::Empty).expect("static 500")
		}
	};

	let (rparts, mut rbody) = response.into_parts();
	let resp_for_h3 = http::Response::from_parts(rparts, ());
	if let Err(e) = send_stream.send_response(resp_for_h3).await {
		tracing::debug!(error = %e, conn_id = %conn.id, "h3 send_response failed");
		return;
	}
	loop {
		let frame = std::future::poll_fn(|cx| Pin::new(&mut rbody).poll_frame(cx)).await;
		match frame {
			Some(Ok(f)) => {
				if let Some(data) = f.data_ref()
					&& let Err(e) = send_stream.send_data(data.clone()).await
				{
					tracing::debug!(error = %e, conn_id = %conn.id, "h3 send_data failed");
					return;
				} else if let Some(trailers) = f.trailers_ref()
					&& let Err(e) = send_stream.send_trailers(trailers.clone()).await
				{
					tracing::debug!(error = %e, conn_id = %conn.id, "h3 send_trailers failed");
					return;
				}
			}
			Some(Err(e)) => {
				tracing::debug!(error = %e, conn_id = %conn.id, "h3 response body err");
				return;
			}
			None => break,
		}
	}
	if let Err(e) = send_stream.finish().await {
		tracing::debug!(error = %e, conn_id = %conn.id, "h3 finish failed");
	}
}
