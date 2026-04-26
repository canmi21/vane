//! `Node::Upgrade` execution — L4 → L7 bridge. Hands a byte stream
//! (plain TCP or TLS-terminated) to `hyper::server::conn::http1::Builder`;
//! each decoded `Request` walks the L7 sub-graph from the `Upgrade.next`
//! node.
//!
//! See `spec/architecture/06-l4.md` § _L4 → L7 upgrade_,
//! `spec/architecture/02-flow.md` § _Execution model_ (Upgrade arm).
//! Feature: S1-17.
//!
//! Out of MVP scope (separately tracked): H2 / H3 / WS-101.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
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
#[allow(clippy::too_many_lines)]
pub(crate) async fn drive_h1_server<S>(
	stream: S,
	graph: Arc<FlowGraph>,
	l7_entry: NodeId,
	conn: Arc<ConnContext>,
	log: Arc<dyn FlowLogSink>,
	cancel: CancellationToken,
	verbosity: FlowLogVerbosity,
) -> Result<ExecutorOutput, Error>
where
	S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
	// Record negotiated HTTP version once on the shared ConnContext so L7
	// predicates / middleware can read it. H1 only this round.
	let _ = conn.http_version.set(HttpVersion::Http1_1);

	let svc = service_fn(move |mut req: hyper::Request<Incoming>| {
		let graph = Arc::clone(&graph);
		let conn = Arc::clone(&conn);
		let log = Arc::clone(&log);
		let cancel = cancel.clone();
		async move {
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

			let span = tracing::info_span!(
				"request",
				conn = %conn.id,
				method = %vane_req.method(),
				path = %vane_req.uri().path(),
			);

			let mut ctx = FlowCtx {
				span,
				log,
				cancel,
				verbosity,
				trajectory: TrajectoryBuilder::new(conn.id, l7_entry, unix_ms_now()),
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
					tokio::spawn(async move {
						match client_on_upgrade.await {
							Ok(upgraded) => {
								let mut client_io = TokioIo::new(upgraded);
								if let Err(e) = copy_bidirectional(&mut client_io, &mut *upstream_io).await {
									tracing::debug!(
										?e,
										%conn_id,
										"ws byte tunnel ended with io error",
									);
								}
							}
							Err(e) => tracing::warn!(
								?e,
								%conn_id,
								"client ws upgrade await failed",
							),
						}
					});
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
	hyper::server::conn::http1::Builder::new()
		.serve_connection(io, svc)
		.with_upgrades()
		.await
		.map_err(|e| Error::protocol("h1 serve_connection").with_source(e))?;

	Ok(ExecutorOutput::Closed)
}

fn unix_ms_now() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
		.unwrap_or_default()
}
