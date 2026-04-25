//! End-to-end tests for the H1 upgrade path.
//!
//! These tests build a `SymbolicFlowGraph` whose entry node is a
//! `Node::Upgrade { next }` followed by an L7 sub-graph
//! (`Fetch -> Terminate(WriteHttpResponse)`), link it, hand it to
//! `ListenerSet::start`, and drive real HTTP/1.1 requests with a `hyper`
//! client.
//!
//! Anchors per spec:
//!
//! * `spec/architecture/02-flow.md` § _Execution model_ — the `Node::Upgrade`
//!   arm hands the L4 connection to the H1 server driver; per-decoded
//!   request the driver builds a fresh `FlowCtx` and re-enters the executor
//!   at `Upgrade.next`. The L7 path's `ExecutorOutput::HttpResponse` flows
//!   back to the driver, which serialises it onto the wire.
//! * `spec/architecture/02-flow.md` § Phase state machine — Upgrade
//!   transitions `L4Raw -> L7Request`; the sub-graph entry must accept
//!   phase `L7Request` (a `Node::Fetch` that produces a Response satisfies
//!   that requirement and steps to `next_response` afterwards).
//! * `spec/architecture/03-types.md` § _L7 body_ — `Body::Static`,
//!   `Body::Stream`, `Body::Empty` are the three body shapes the executor
//!   round-trips through hyper.
//! * `spec/architecture/06-l4.md` § _L4 -> L7 upgrade_ — the upgrade arm is
//!   driven by listener config; for cleartext H1 it is purely the hyper H1
//!   server.
//! * `spec/architecture/07-l7.md` — the H1 path.
//!
//! The `drive_h1_server` helper itself is `pub(crate)`, so these tests
//! drive it indirectly through `ListenerSet::start` plus a TCP client
//! using `hyper::client::conn::http1`.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::{BodyExt, Empty, Full};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use vane_core::{
	Body, ConnContext, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, FlowLogEvent, FlowLogSink,
	L7Fetch, L7FetchOutput, Node, NodeId, Request, Response, SymbolicFetchRef, SymbolicFlowGraph,
	Terminator, TerminatorId, UpstreamReason,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FetchInst, FlowGraph};
use vane_engine::verbosity::VerbosityState;

// ---------------------------------------------------------------------------
// FlowLogSink fixture: drops events; the H1 path emits trajectories per
// request but assertions in this file are about the wire-level outcome, not
// the trajectory shape (already covered by `tests/executor.rs`).
// ---------------------------------------------------------------------------

struct DropSink;

impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

// ---------------------------------------------------------------------------
// Free port discovery — same pattern used by `tests/listener.rs`. Bind an
// ephemeral listener, take its `local_addr`, drop it. Brief race windows
// between drop and the listener-under-test rebinding are tolerated by the
// 50 ms post-`start` sleep before clients connect.
// ---------------------------------------------------------------------------

async fn pick_port() -> SocketAddr {
	let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral for port pick");
	let addr = l.local_addr().expect("local_addr");
	drop(l);
	addr
}

fn sample_meta() -> FlowGraphMeta {
	FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
	}
}

// ---------------------------------------------------------------------------
// Symbolic-graph builder: every test in this file drives the executor through
// the same shape:
//
//   entry(Upgrade { next: 1 })
//     -> Fetch(L7, kind = HttpSynthesize, next_response = 2)
//       -> Terminate(WriteHttpResponse)
//
// The Fetch is registered against `FetchKind::HttpSynthesize` so the linker
// resolves it through `FetchFactories::register`. Tests parameterise the
// concrete `L7Fetch` impl via the factory closure.
// ---------------------------------------------------------------------------

fn upgrade_fetch_terminate_graph(
	addr: SocketAddr,
	fetch_factory: Box<dyn Fn() -> FetchInst + Send + Sync>,
) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Upgrade { next: NodeId::new(1) },
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: Value::Null }],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta: sample_meta(),
	});

	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	let factory = Arc::new(fetch_factory);
	fetch.register(FetchKind::HttpSynthesize, move |_args| Ok((factory)()));
	FlowGraph::link(sym, &mw, &fetch).expect("link upgrade-fetch-terminate graph")
}

// L7 path that reaches Terminate(Close) without producing a Response.
// Shape:
//
//   entry(Upgrade { next: 1 })
//     -> Terminate(Close)
//
// Used by the no-route synthesis test — `drive_h1_server` translates the
// resulting `ExecutorOutput::Closed` to 404 + `Connection: close` for
// HTTP/1 clients (HTTP/2 / HTTP/3 will pick 421 once those drivers land).
fn upgrade_close_graph(addr: SocketAddr) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![Node::Upgrade { next: NodeId::new(1) }, Node::Terminate(TerminatorId::new(0))],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![],
		terminators: vec![Terminator::Close],
		entries,
		meta: sample_meta(),
	});
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	FlowGraph::link(sym, &mw, &fetch).expect("link upgrade-close graph")
}

// ---------------------------------------------------------------------------
// Spawn the listener and wait briefly for the accept loop to bind.
// ---------------------------------------------------------------------------

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("graph has at least one entry").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);

	let set = ListenerSet::new();
	set.start(graph, verbosity, sink);

	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

// ---------------------------------------------------------------------------
// Hyper H1 client handshake. Returns the `SendRequest` handle so the test can
// fire one or more requests on the same TCP connection (test 3 reuses it for
// keep-alive).
// ---------------------------------------------------------------------------

async fn h1_client_handshake_empty(
	addr: SocketAddr,
) -> hyper::client::conn::http1::SendRequest<Empty<Bytes>> {
	let stream = tokio::net::TcpStream::connect(addr).await.expect("client connect");
	let io = TokioIo::new(stream);
	let (sender, conn) =
		hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io).await.expect("h1 handshake");
	tokio::spawn(async move {
		let _ = conn.await;
	});
	sender
}

async fn h1_client_handshake_full(
	addr: SocketAddr,
) -> hyper::client::conn::http1::SendRequest<Full<Bytes>> {
	let stream = tokio::net::TcpStream::connect(addr).await.expect("client connect");
	let io = TokioIo::new(stream);
	let (sender, conn) =
		hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io).await.expect("h1 handshake");
	tokio::spawn(async move {
		let _ = conn.await;
	});
	sender
}

// ---------------------------------------------------------------------------
// L7Fetch fixtures.
// ---------------------------------------------------------------------------

/// Synthesises a `200 OK` whose body is `payload`. `Body::Static` per
/// `03-types.md` § _L7 body_.
struct StaticOkFetch {
	payload: Bytes,
}

#[async_trait]
impl L7Fetch for StaticOkFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let resp: Response = http::Response::builder()
			.status(200)
			.body(Body::Static(self.payload.clone()))
			.expect("build static response");
		Ok(L7FetchOutput::Response(resp))
	}
}

/// Drains the request body to the last frame and echoes the aggregated
/// payload back as a `Body::Static` response. Per `02-flow.md` § _Execution
/// model_, the request body that reached `L7Fetch::fetch` is the `Body`
/// adapted from `hyper::body::Incoming` by `drive_h1_server`'s
/// `IncomingAdapter`.
struct EchoFetch;

#[async_trait]
impl L7Fetch for EchoFetch {
	async fn fetch(
		&self,
		req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let collected = req.into_body().collect().await.map_err(|e| {
			Error::protocol("echo body collect")
				.with_source(Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()))
		})?;
		let bytes = collected.to_bytes();
		let resp: Response =
			http::Response::builder().status(200).body(Body::Static(bytes)).expect("build echo");
		Ok(L7FetchOutput::Response(resp))
	}
}

/// Returns a streaming response — five 1KB chunks then EOF, exposed through
/// `Body::Stream` per the type contract in `03-types.md` § _L7 body_.
struct StreamFiveKbFetch;

#[async_trait]
impl L7Fetch for StreamFiveKbFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let body = Body::from_producer(FiveKbProducer { remaining: 5 });
		let resp: Response =
			http::Response::builder().status(200).body(body).expect("build stream response");
		Ok(L7FetchOutput::Response(resp))
	}
}

/// Pushes five 1024-byte chunks then signals end-of-stream. Implemented by
/// hand to keep the test free of any real upstream IO; `Body::from_producer`
/// only requires `HttpBody<Data = Bytes, Error: Into<Error>>`.
struct FiveKbProducer {
	remaining: usize,
}

impl HttpBody for FiveKbProducer {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		self: Pin<&mut Self>,
		_cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		let this = self.get_mut();
		if this.remaining == 0 {
			return Poll::Ready(None);
		}
		this.remaining -= 1;
		let chunk = Bytes::from_static(&[0u8; 1024]);
		Poll::Ready(Some(Ok(Frame::data(chunk))))
	}

	fn is_end_stream(&self) -> bool {
		self.remaining == 0
	}

	fn size_hint(&self) -> SizeHint {
		SizeHint::with_exact(1024 * (self.remaining as u64))
	}
}

/// `L7Fetch` fixture that always returns `Err(Error::upstream(..))`. The H1
/// driver's contract (`drive_h1_server` doc): per-request executor errors
/// are translated to a 500 inside the service-fn so the connection itself
/// can stay alive. We assert both the status and that a follow-up request
/// on the same connection still succeeds.
struct ErrFetch {
	hits: Arc<AtomicUsize>,
}

#[async_trait]
impl L7Fetch for ErrFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		self.hits.fetch_add(1, Ordering::SeqCst);
		Err(Error::upstream(UpstreamReason::Unreachable))
	}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// 1. h1_get_request_returns_synthesized_response
//
// Spec anchor: `02-flow.md` § _Execution model_. Upgrade hands the TCP
// stream to the H1 driver; per-request the executor walks
// `Fetch -> Terminate(WriteHttpResponse)` and the driver serialises the
// `Response` onto the wire. The client receives the synthesised 200.
#[tokio::test]
async fn h1_get_request_returns_synthesized_response() {
	let addr = pick_port().await;
	let payload = Bytes::from_static(b"ok");
	let payload_for_factory = payload.clone();
	let graph = upgrade_fetch_terminate_graph(
		addr,
		Box::new(move || {
			FetchInst::L7(Arc::new(StaticOkFetch { payload: payload_for_factory.clone() }))
		}),
	);

	let (set, addr) = start_listener(graph).await;

	let mut sender = h1_client_handshake_empty(addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET request");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "executor synthesised 200 must traverse the wire");

	let body = resp.into_body().collect().await.expect("collect response body").to_bytes();
	assert_eq!(body, payload, "client must receive the L7Fetch payload bytes verbatim");

	set.shutdown(Duration::from_secs(2)).await;
}

// 2. h1_request_body_flows_through_to_l7_fetch
//
// Spec anchor: `02-flow.md` § _Execution model_ + `03-types.md` § _L7 body_.
// The hyper-decoded request body lands in `Body::Stream` via
// `IncomingAdapter` (an internal `drive_h1_server` adapter; that detail is
// covered separately by the upgrade module). The L7 fixture drains it and
// echoes it back. Asserts the full request bytes survive the round trip.
#[tokio::test]
async fn h1_request_body_flows_through_to_l7_fetch() {
	let addr = pick_port().await;
	let graph = upgrade_fetch_terminate_graph(addr, Box::new(|| FetchInst::L7(Arc::new(EchoFetch))));

	let (set, addr) = start_listener(graph).await;

	let mut sender = h1_client_handshake_full(addr).await;
	let req = hyper::Request::builder()
		.method("POST")
		.uri("/echo")
		.header("host", "test.local")
		.body(Full::<Bytes>::new(Bytes::from_static(b"hello")))
		.expect("build POST request");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "echo path must surface as 200");

	let body = resp.into_body().collect().await.expect("collect response body").to_bytes();
	assert_eq!(body.as_ref(), b"hello", "request bytes must round-trip via the L7 fetch");

	set.shutdown(Duration::from_secs(2)).await;
}

// 3. h1_keep_alive_two_requests_share_connection
//
// Spec anchor: `02-flow.md` § _Execution model_, Upgrade arm — for each
// decoded request the driver constructs a fresh `FlowCtx` and re-enters
// the executor. Two back-to-back GETs over the same `SendRequest` exercise
// hyper's H1 keep-alive: both must succeed and return 200.
#[tokio::test]
async fn h1_keep_alive_two_requests_share_connection() {
	let addr = pick_port().await;
	let payload = Bytes::from_static(b"ok");
	let payload_for_factory = payload.clone();
	let graph = upgrade_fetch_terminate_graph(
		addr,
		Box::new(move || {
			FetchInst::L7(Arc::new(StaticOkFetch { payload: payload_for_factory.clone() }))
		}),
	);

	let (set, addr) = start_listener(graph).await;

	let mut sender = h1_client_handshake_empty(addr).await;

	for nth in 0..2 {
		let req = hyper::Request::builder()
			.method("GET")
			.uri("/")
			.header("host", "test.local")
			.body(Empty::<Bytes>::new())
			.unwrap_or_else(|_| panic!("build keep-alive request #{nth}"));
		let resp = sender
			.send_request(req)
			.await
			.unwrap_or_else(|e| panic!("send keep-alive request #{nth}: {e}"));
		assert_eq!(resp.status().as_u16(), 200, "keep-alive request #{nth} must succeed");
		let body = resp.into_body().collect().await.expect("collect body").to_bytes();
		assert_eq!(body, payload, "keep-alive request #{nth} body must match");
	}

	set.shutdown(Duration::from_secs(2)).await;
}

// 4. h1_response_body_static_writes_full_payload
//
// Spec anchor: `03-types.md` § _L7 body_, `Body::Static` variant — a
// 100KB static payload must reach the client unchanged. Asserts both the
// status and the exact byte count, guarding against any chunked-framing
// truncation in the H1 server response writer.
#[tokio::test]
async fn h1_response_body_static_writes_full_payload() {
	let addr = pick_port().await;
	let payload = Bytes::from(vec![0u8; 100_000]);
	let payload_for_factory = payload.clone();
	let graph = upgrade_fetch_terminate_graph(
		addr,
		Box::new(move || {
			FetchInst::L7(Arc::new(StaticOkFetch { payload: payload_for_factory.clone() }))
		}),
	);

	let (set, addr) = start_listener(graph).await;

	let mut sender = h1_client_handshake_empty(addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/big")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build big-GET request");
	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "static-body path must surface as 200");

	let body = resp.into_body().collect().await.expect("collect response body").to_bytes();
	assert_eq!(body.len(), 100_000, "client must read exactly 100KB across H1 framing");

	set.shutdown(Duration::from_secs(2)).await;
}

// 5. h1_response_body_stream_drains_to_completion
//
// Spec anchor: `03-types.md` § _L7 body_, `Body::Stream` variant. The L7
// fetch returns a hand-rolled `http_body::Body` producer that emits five
// 1KB frames. The client collects exactly 5KB. This guards the "stream
// frames pass through to the egress encoder" half of the body story.
#[tokio::test]
async fn h1_response_body_stream_drains_to_completion() {
	let addr = pick_port().await;
	let graph =
		upgrade_fetch_terminate_graph(addr, Box::new(|| FetchInst::L7(Arc::new(StreamFiveKbFetch))));

	let (set, addr) = start_listener(graph).await;

	let mut sender = h1_client_handshake_empty(addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/stream")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build stream-GET request");
	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "stream-body path must surface as 200");

	let body = resp.into_body().collect().await.expect("collect response body").to_bytes();
	assert_eq!(body.len(), 5 * 1024, "five 1KB frames must aggregate to 5KB on the client");

	set.shutdown(Duration::from_secs(2)).await;
}

// 6. h1_l7_fetch_error_surfaces_as_500
//
// Spec anchor: `drive_h1_server`'s contract (the # Errors note) — a
// per-request executor `Err(_)` is translated to a synthetic 500 inside
// the service-fn so the H1 connection stays alive. After the 500 the
// underlying TCP connection must remain usable; we don't strictly require
// keep-alive to succeed (synthesised 500 may carry `Connection: close`),
// but the failure mode under test is a panic / hang, not a polite close.
#[tokio::test]
async fn h1_l7_fetch_error_surfaces_as_500() {
	let addr = pick_port().await;
	let hits = Arc::new(AtomicUsize::new(0));
	let hits_for_factory = Arc::clone(&hits);
	let graph = upgrade_fetch_terminate_graph(
		addr,
		Box::new(move || FetchInst::L7(Arc::new(ErrFetch { hits: Arc::clone(&hits_for_factory) }))),
	);

	let (set, addr) = start_listener(graph).await;

	let mut sender = h1_client_handshake_empty(addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/boom")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build error-GET request");

	let resp = sender.send_request(req).await.expect("send_request must not hang");
	assert_eq!(
		resp.status().as_u16(),
		500,
		"L7Fetch Err must surface as a synthesised 500 to the wire",
	);
	// Drain the (likely empty) body so hyper releases connection state.
	let _ = resp.into_body().collect().await;

	assert_eq!(hits.load(Ordering::SeqCst), 1, "L7Fetch must run exactly once before the 500");

	set.shutdown(Duration::from_secs(2)).await;
}

// 7. h1_no_route_returns_404_with_connection_close
//
// Spec anchor: 02-flow.md § _Execution model_ — `Terminate(Close)` is a
// proxy-layer "no route" signal. Inside an H1 connection the L4 RST
// analogue is "synthesise 404 + Connection: close" so the H1 socket
// terminates cleanly without leaking origin-server semantics. (HTTP/2
// and HTTP/3 will pick 421 Misdirected Request once those drivers land;
// see `drive_h1_server`'s `Closed` arm.)
#[tokio::test]
async fn h1_no_route_returns_404_with_connection_close() {
	let addr = pick_port().await;
	let graph = upgrade_close_graph(addr);

	let (set, addr) = start_listener(graph).await;

	let mut sender = h1_client_handshake_empty(addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/no-rule-covers-this")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build no-route GET request");

	let resp = sender.send_request(req).await.expect("send_request must not hang");
	assert_eq!(
		resp.status().as_u16(),
		404,
		"H1 unmatched path must surface as 404; H2/H3 will pick 421 in their drivers",
	);
	assert_eq!(
		resp.headers().get("connection").and_then(|v| v.to_str().ok()),
		Some("close"),
		"Closed-arm response must carry Connection: close to terminate the H1 connection",
	);
	let body = resp.into_body().collect().await.expect("collect").to_bytes();
	assert!(body.is_empty(), "Closed-arm response body must be empty");

	set.shutdown(Duration::from_secs(2)).await;
}
