//! `Node::Upgrade` execution — L4 → L7 bridge. Hands the TCP stream to
//! `hyper::server::conn::http1::Builder`; each decoded `Request` walks
//! the L7 sub-graph from the `Upgrade.next` node.
//!
//! See `spec/architecture/06-l4.md` § _L4 → L7 upgrade_,
//! `spec/architecture/02-flow.md` § _Execution model_ (Upgrade arm).
//! Feature: S1-17.
//!
//! Out of MVP scope (separately tracked): H2 / H3 / WS-101 / TLS / ALPN.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use vane_core::{
	Body, ConnContext, Error, FlowCtx, FlowLogSink, FlowLogVerbosity, HttpVersion, NodeId, Request,
	Response, TrajectoryBuilder,
};

use crate::executor::{ExecutorInput, ExecutorOutput, execute};
use crate::flow_graph::FlowGraph;

/// Drive a `TcpStream` as an H1 server. For each decoded request, build a
/// fresh `FlowCtx` (sharing `log` / `cancel` / `verbosity` from the outer
/// L4 ctx, with its own `TrajectoryBuilder`) and call the executor with
/// the L7 sub-graph entry. The executor's `ExecutorOutput::HttpResponse`
/// flows back to the service-fn, which returns it to hyper for wire
/// serialisation.
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
pub(crate) async fn drive_h1_server(
	stream: TcpStream,
	graph: Arc<FlowGraph>,
	l7_entry: NodeId,
	conn: Arc<ConnContext>,
	log: Arc<dyn FlowLogSink>,
	cancel: CancellationToken,
	verbosity: FlowLogVerbosity,
) -> Result<ExecutorOutput, Error> {
	// Record negotiated HTTP version once on the shared ConnContext so L7
	// predicates / middleware can read it. H1 only this round.
	let _ = conn.http_version.set(HttpVersion::Http1_1);

	let svc = service_fn(move |req: hyper::Request<Incoming>| {
		let graph = Arc::clone(&graph);
		let conn = Arc::clone(&conn);
		let log = Arc::clone(&log);
		let cancel = cancel.clone();
		async move {
			let vane_req: Request =
				req.map(|incoming| Body::Stream(Box::pin(IncomingAdapter { inner: Box::pin(incoming) })));

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
				Ok(ExecutorOutput::HttpResponse(r)) => Ok::<Response, std::convert::Infallible>(r),
				Ok(ExecutorOutput::Closed) => {
					// L7 path ended via Terminate(Close) without producing a
					// Response. Synthesise a 204 — the client expects some
					// HTTP reply on a decoded request.
					Ok(http::Response::builder().status(204).body(Body::Empty).expect("static"))
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
	hyper::server::conn::http1::Builder::new()
		.serve_connection(io, svc)
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

/// Adapts `hyper::body::Incoming` into the `HttpBody<Data = Bytes,
/// Error = vane_core::Error>` shape required by `vane_core::Body::Stream`.
/// `inner` is `Pin<Box<Incoming>>` rather than `Incoming` so we can poll
/// without unsafe pin projection (CLAUDE.md `unsafe_code = "deny"` —
/// same pattern as `vane_core::body::BodyStreamAdapter`).
struct IncomingAdapter {
	inner: Pin<Box<Incoming>>,
}

impl HttpBody for IncomingAdapter {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		match self.get_mut().inner.as_mut().poll_frame(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Ready(Some(Ok(f))) => Poll::Ready(Some(Ok(f))),
			Poll::Ready(Some(Err(e))) => {
				Poll::Ready(Some(Err(Error::protocol("h1 incoming body").with_source(e))))
			}
		}
	}

	fn is_end_stream(&self) -> bool {
		self.inner.is_end_stream()
	}

	fn size_hint(&self) -> SizeHint {
		self.inner.size_hint()
	}
}
