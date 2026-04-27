//! Integration tests for `vane_engine::fetch::http_proxy`.
//!
//! Covers the H1 → H1 cleartext reverse-proxy contract described in
//! `spec/architecture/07-l7.md` § _H1 path_ and
//! `spec/architecture/05-terminator.md` § _`HttpProxy`_:
//!
//! * The Fetch rewrites the request's scheme + authority to point at the
//!   configured `upstream` while preserving path and query verbatim
//!   (`hyper_util::Client` routes by URI authority — see 07-l7.md
//!   "TCP pooling is delegated entirely to `hyper_util`'s `Client`, which
//!   keys its internal pool by authority").
//! * Request headers and request body flow through to the upstream
//!   unchanged at the L7-Fetch boundary.
//! * Upstream response bodies are always exposed as `Body::Stream(...)`
//!   per 07-l7.md § _`HttpProxyFetch` commits to streaming response
//!   bodies_, so multi-frame upstream responses round-trip without any
//!   defensive collection.
//! * Unreachable upstreams surface as `Err(Error::upstream(Unreachable))`
//!   inside the `L7Fetch`; the H1 driver translates per-request executor
//!   errors into a synthetic 500 (see `drive_h1_server`'s contract,
//!   exercised end-to-end by `tests/hyper_upgrade.rs`'s
//!   `h1_l7_fetch_error_surfaces_as_500`).
//! * The factory rejects a missing `upstream` arg up-front per the
//!   docstring on `vane_engine::fetch::http_proxy::factory`.
//!
//! Each test wires a tokio-driven hyper "upstream" server on an ephemeral
//! port, then a vane `ListenerSet` whose graph is
//! `Upgrade -> Fetch(HttpProxy{upstream}) -> Terminate(WriteHttpResponse)`.
//! The vane factory pattern is identical to the L4-forward integration
//! tests; only the graph shape changes (Upgrade-prefixed L7 path).

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::{BodyExt, Empty, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, Node, NodeId, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FactoryError, FetchFactories, MiddlewareFactories};
use vane_engine::fetch::http_proxy::{
	factory as http_proxy_factory, register as register_http_proxy,
};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

// ---------------------------------------------------------------------------
// FlowLogSink fixture: drops events. These tests assert wire-level outcomes
// (status code, body bytes, observed upstream-side state); trajectory shape
// is covered by `tests/executor.rs` and `tests/listener.rs`.
// ---------------------------------------------------------------------------

struct DropSink;

impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

// ---------------------------------------------------------------------------
// Free-port discovery — bind ephemeral, take `local_addr()`, drop. Same
// pattern as `tests/hyper_upgrade.rs` / `tests/listener.rs`.
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
		short_circuit_response_entry: std::collections::BTreeMap::new(),
		listener_tls: std::collections::BTreeMap::new(),
		listener_kinds: std::collections::BTreeMap::new(),
	}
}

// ---------------------------------------------------------------------------
// Symbolic graph factory: every test in this file drives:
//
//   entry(Upgrade { next: 1 })
//     -> Fetch { id: 0, kind = HttpProxy{upstream}, next_response: 2 }
//       -> Terminate(WriteHttpResponse)
//
// Per `02-flow.md` § _Phase state machine_, the Upgrade arm transitions
// the executor to L7Request, satisfying `Node::Fetch`'s phase precondition.
// ---------------------------------------------------------------------------

fn proxy_graph(listen: SocketAddr, upstream: &str) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Upgrade { next: NodeId::new(1) },
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef {
			kind: FetchKind::HttpProxy,
			args: serde_json::json!({ "upstream": upstream }),
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta: sample_meta(),
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	register_http_proxy(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link http_proxy graph")
}

// ---------------------------------------------------------------------------
// Spawn the listener and wait briefly for the accept loop to bind. Mirrors
// the helper in `tests/hyper_upgrade.rs`.
// ---------------------------------------------------------------------------

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("graph has at least one entry").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);

	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(graph)), verbosity, sink);

	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

// ---------------------------------------------------------------------------
// Hyper H1 client handshake. Spawns the connection task so the caller can
// fire a `send_request`. Two flavours (Empty / Full) match the two body
// shapes the tests need.
// ---------------------------------------------------------------------------

async fn h1_client_empty(
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

async fn h1_client_full(addr: SocketAddr) -> hyper::client::conn::http1::SendRequest<Full<Bytes>> {
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
// Upstream fixture: a hyper H1 server bound to an ephemeral port, driven by
// a per-connection service-fn. Returns the bound `SocketAddr` so the test
// can wire `proxy_graph(.., upstream_addr.to_string())`. The accept loop
// runs until the test's tokio runtime tears down at scope exit.
// ---------------------------------------------------------------------------

async fn spawn_upstream<S, B, E>(svc: S) -> SocketAddr
where
	S: Fn(
			hyper::Request<hyper::body::Incoming>,
		)
			-> std::pin::Pin<Box<dyn std::future::Future<Output = Result<hyper::Response<B>, E>> + Send>>
		+ Send
		+ Sync
		+ Clone
		+ 'static,
	B: HttpBody<Data = Bytes> + Send + 'static,
	B::Error: std::error::Error + Send + Sync + 'static,
	E: std::error::Error + Send + Sync + 'static,
{
	let listener =
		tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind upstream listener");
	let addr = listener.local_addr().expect("upstream local_addr");
	tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			let svc = svc.clone();
			tokio::spawn(async move {
				let io = TokioIo::new(sock);
				let _ =
					hyper::server::conn::http1::Builder::new().serve_connection(io, service_fn(svc)).await;
			});
		}
	});
	addr
}

// ---------------------------------------------------------------------------
// 1. http_proxy_forwards_get_to_upstream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_proxy_forwards_get_to_upstream() {
	// 05-terminator.md § _`HttpProxy`_: the Fetch produces a Response by
	// forwarding the client's Request to the configured upstream. Asserting
	// the upstream's body bytes survive the round trip is the minimum
	// "the bridge is wired" check.
	let upstream_addr = spawn_upstream(|_req| {
		Box::pin(async move {
			Ok::<_, Infallible>(
				hyper::Response::builder()
					.status(200)
					.body(Full::<Bytes>::new(Bytes::from_static(b"hello from upstream")))
					.expect("build upstream response"),
			)
		})
	})
	.await;

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(proxy_addr, &upstream_addr.to_string());
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET request");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "upstream 200 must surface verbatim");
	let body = resp.into_body().collect().await.expect("collect response body").to_bytes();
	assert_eq!(body.as_ref(), b"hello from upstream", "upstream body must round-trip byte-for-byte");

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_millis(500)).await;
}

// ---------------------------------------------------------------------------
// 2. http_proxy_preserves_request_headers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_proxy_preserves_request_headers() {
	// 07-l7.md § _H1 path_ + 05-terminator.md § _`HttpProxy`_: forwarding
	// preserves the request's headers up to the URI rewrite (scheme +
	// authority). Custom headers must reach the upstream untouched, and
	// the upstream's response headers must reach the client untouched.
	let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
	let observed_for_svc = Arc::clone(&observed);
	let upstream_addr = spawn_upstream(move |req| {
		let observed = Arc::clone(&observed_for_svc);
		Box::pin(async move {
			let header_val =
				req.headers().get("x-custom-header").and_then(|v| v.to_str().ok()).map(str::to_string);
			*observed.lock() = header_val.clone();
			let echoed = header_val.unwrap_or_default();
			Ok::<_, Infallible>(
				hyper::Response::builder()
					.status(200)
					.header("x-echoed-custom", echoed)
					.body(Empty::<Bytes>::new())
					.expect("build upstream response"),
			)
		})
	})
	.await;

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(proxy_addr, &upstream_addr.to_string());
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.header("x-custom-header", "foo")
		.body(Empty::<Bytes>::new())
		.expect("build header-carrying GET");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "header pass-through path must surface as 200");
	let echoed =
		resp.headers().get("x-echoed-custom").and_then(|v| v.to_str().ok()).map(str::to_string);
	assert_eq!(
		echoed.as_deref(),
		Some("foo"),
		"upstream-side response headers must reach the client unchanged",
	);

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_millis(500)).await;

	assert_eq!(
		observed.lock().as_deref(),
		Some("foo"),
		"upstream must observe the client's X-Custom-Header verbatim",
	);
}

// ---------------------------------------------------------------------------
// 3. http_proxy_streams_response_body
// ---------------------------------------------------------------------------

/// Hand-rolled `http_body::Body` emitting `chunks` separate 1KB frames.
/// Avoids pulling in a streams crate not in the engine's dev-dependencies.
struct OneKbFramesBody {
	remaining: usize,
}

impl HttpBody for OneKbFramesBody {
	type Data = Bytes;
	type Error = Infallible;

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

#[tokio::test]
async fn http_proxy_streams_response_body() {
	// 07-l7.md § _`HttpProxyFetch` commits to streaming response bodies_:
	// upstream response bodies are returned as `Body::Stream(...)`. A
	// multi-frame upstream body must therefore reach the client without
	// being collected and re-emitted as a single static block. The client
	// drains via `BodyExt::collect` and we assert exact byte count — five
	// 1KB frames must aggregate to 5KB, end-to-end.
	let upstream_addr = spawn_upstream(|_req| {
		Box::pin(async move {
			Ok::<_, Infallible>(
				hyper::Response::builder()
					.status(200)
					.body(OneKbFramesBody { remaining: 5 })
					.expect("build streaming upstream response"),
			)
		})
	})
	.await;

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(proxy_addr, &upstream_addr.to_string());
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/stream")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build streaming-GET");
	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "streaming proxy path must surface as 200");

	let body = resp.into_body().collect().await.expect("collect response body").to_bytes();
	assert_eq!(body.len(), 5 * 1024, "five 1KB upstream frames must aggregate to 5KB on the client");

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_millis(500)).await;
}

// ---------------------------------------------------------------------------
// 4. http_proxy_post_body_flows_to_upstream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_proxy_post_body_flows_to_upstream() {
	// 07-l7.md § _Body streaming across versions_: request-body frames
	// reach the upstream encoder via `http_body::Body::poll_frame` without
	// vane-layer copy. The upstream-side service draining the request body
	// in full and echoing it confirms the request body survives the
	// round trip exactly.
	let observed: Arc<Mutex<Option<Bytes>>> = Arc::new(Mutex::new(None));
	let observed_for_svc = Arc::clone(&observed);
	let upstream_addr = spawn_upstream(move |req| {
		let observed = Arc::clone(&observed_for_svc);
		Box::pin(async move {
			let collected = req.into_body().collect().await.expect("upstream collect body").to_bytes();
			*observed.lock() = Some(collected.clone());
			Ok::<_, Infallible>(
				hyper::Response::builder()
					.status(200)
					.body(Full::<Bytes>::new(collected))
					.expect("build echoed response"),
			)
		})
	})
	.await;

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(proxy_addr, &upstream_addr.to_string());
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_full(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("POST")
		.uri("/echo")
		.header("host", "test.local")
		.body(Full::<Bytes>::new(Bytes::from_static(b"client-payload")))
		.expect("build POST request");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "echo path must surface as 200");
	let body = resp.into_body().collect().await.expect("collect response body").to_bytes();
	assert_eq!(
		body.as_ref(),
		b"client-payload",
		"upstream's echoed body must match the client's POST payload",
	);

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_millis(500)).await;

	let observed_payload = observed.lock().clone();
	assert_eq!(
		observed_payload.as_deref(),
		Some(b"client-payload".as_slice()),
		"upstream must observe the request body bytes verbatim",
	);
}

// ---------------------------------------------------------------------------
// 5. http_proxy_unreachable_upstream_surfaces_as_500_via_h1_driver
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_proxy_unreachable_upstream_surfaces_as_500_via_h1_driver() {
	// 05-terminator.md § _Failure modes_: an unreachable upstream produces
	// `Err(Error::upstream(Unreachable))` from `L7Fetch::fetch`. The H1
	// driver translates per-request executor errors into a synthesised
	// 500 response so the H1 connection itself stays alive — see
	// `drive_h1_server`'s contract, also exercised by
	// `tests/hyper_upgrade.rs::h1_l7_fetch_error_surfaces_as_500`.
	//
	// Picking an unbound address: bind ephemeral, take addr, drop. There
	// is a tiny reuse window on darwin; the spec contract is "unreachable
	// upstream → executor Err(_) → driver-synthesised 500", and any
	// surfaced 500 satisfies it regardless of the specific UpstreamReason
	// that triggered the connect failure.
	let unreachable = pick_port().await;
	let proxy_addr = pick_port().await;
	let graph = proxy_graph(proxy_addr, &unreachable.to_string());
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/boom")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build error-GET");

	let resp = sender.send_request(req).await.expect("send_request must not hang");
	assert_eq!(
		resp.status().as_u16(),
		500,
		"unreachable upstream must surface as a synthesised 500 to the wire",
	);
	// Drain (likely empty) body so hyper releases its connection state.
	let _ = resp.into_body().collect().await;

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_millis(500)).await;
}

// ---------------------------------------------------------------------------
// 6. http_proxy_factory_rejects_missing_upstream_arg
// ---------------------------------------------------------------------------

#[test]
fn http_proxy_factory_rejects_missing_upstream_arg() {
	// Per the public docstring on `http_proxy::factory`: missing `upstream`
	// yields a `FactoryError` whose message references the offending field.
	// Using let-else because `FetchInst` does not implement `Debug`; we
	// cannot rely on `assert!(matches!(_, Err(_)))`-style helpers that
	// would print the unexpected `Ok(_)` payload.
	let Err(FactoryError(msg)) = http_proxy_factory(&serde_json::json!({})) else {
		panic!("missing upstream must error; got Ok(_)");
	};
	assert!(
		msg.contains("upstream"),
		"FactoryError message must reference the offending field; got {msg:?}",
	);
}

// ---------------------------------------------------------------------------
// 7. http_proxy_uri_path_and_query_preserved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_proxy_uri_path_and_query_preserved() {
	// 07-l7.md § _H1 path_: the Fetch rewrites scheme + authority but
	// preserves path and query verbatim — `hyper_util::Client` routes by
	// URI authority, the rest is forwarded as-is. The upstream observes
	// the request line's path-and-query exactly as the client wrote it.
	let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
	let observed_for_svc = Arc::clone(&observed);
	let upstream_addr = spawn_upstream(move |req| {
		let observed = Arc::clone(&observed_for_svc);
		Box::pin(async move {
			let pq = req.uri().path_and_query().map(|p| p.as_str().to_string());
			*observed.lock() = pq;
			Ok::<_, Infallible>(
				hyper::Response::builder()
					.status(200)
					.body(Empty::<Bytes>::new())
					.expect("build upstream response"),
			)
		})
	})
	.await;

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(proxy_addr, &upstream_addr.to_string());
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/api/v1?x=1&y=2")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build path+query GET");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "path+query forward path must surface as 200");
	let _ = resp.into_body().collect().await;

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_millis(500)).await;

	assert_eq!(
		observed.lock().as_deref(),
		Some("/api/v1?x=1&y=2"),
		"upstream must observe path and query verbatim — only scheme+authority are rewritten",
	);
}
