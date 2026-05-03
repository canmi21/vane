//! End-to-end coverage for `HttpProxyFetch`'s retry policy.
//!
//! Spec: `spec/architecture/05-terminator.md` § _Retry_ +
//! § _Retry buffering_. The retry decision goes through
//! `vane_core::Error::is_retryable()` for every attempt; this file
//! drives the higher-level scenarios (recovery from refused
//! upstreams, opportunistic vs force buffering, POST opt-in,
//! backoff timing, single-attempt non-retryable cases).

#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use arc_swap::ArcSwap;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, Node, NodeId, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::fetch::http_proxy::register as register_http_proxy;
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

async fn pick_port() -> SocketAddr {
	let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral");
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
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls: BTreeMap::new(),
		listener_kinds: BTreeMap::new(),

		listener_transports: BTreeMap::new(),
	}
}

fn proxy_graph(
	listen: SocketAddr,
	args: serde_json::Value,
	retry_buffer_required: bool,
) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));
	let collect_body_before =
		if retry_buffer_required { Some(vane_core::BodySide::Request) } else { None };
	let body_limit = if retry_buffer_required { 8 * 1024 * 1024 } else { 0 };
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Upgrade { next: NodeId::new(1) },
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before,
				body_limit,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef {
			kind: FetchKind::HttpProxy,
			args,
			retry_buffer_required,
			allow_zero_rtt: None,
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

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("entries").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(graph)), verbosity, sink);
	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

async fn h1_send(proxy_addr: SocketAddr, method: &str, body: Option<Bytes>) -> (u16, Bytes) {
	let stream = tokio::net::TcpStream::connect(proxy_addr).await.expect("client connect");
	let io = TokioIo::new(stream);
	let resp_status: u16;
	let resp_body: Bytes;
	if let Some(b) = body {
		let (mut sender, conn) =
			hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io).await.expect("h1 handshake");
		tokio::spawn(async move {
			let _ = conn.await;
		});
		let req = hyper::Request::builder()
			.method(method)
			.uri("/")
			.header("host", "test.local")
			.body(Full::new(b))
			.expect("build");
		let resp = sender.send_request(req).await.expect("send");
		resp_status = resp.status().as_u16();
		resp_body = resp.into_body().collect().await.expect("collect").to_bytes();
	} else {
		let (mut sender, conn) =
			hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io).await.expect("h1 handshake");
		tokio::spawn(async move {
			let _ = conn.await;
		});
		let req = hyper::Request::builder()
			.method(method)
			.uri("/")
			.header("host", "test.local")
			.body(Empty::<Bytes>::new())
			.expect("build");
		let resp = sender.send_request(req).await.expect("send");
		resp_status = resp.status().as_u16();
		resp_body = resp.into_body().collect().await.expect("collect").to_bytes();
	}
	(resp_status, resp_body)
}

/// Spawn an upstream that refuses (drops sockets without TLS / HTTP)
/// for the first `fail_count` accepts, then serves a 200 forever
/// after. Returns the bound address and an `Arc<AtomicUsize>` counter
/// the test can read to assert the number of accepts seen.
async fn spawn_flaky_upstream(fail_count: usize) -> (SocketAddr, Arc<AtomicUsize>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local_addr");
	let accepted = Arc::new(AtomicUsize::new(0));
	let accepted_clone = Arc::clone(&accepted);
	tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			let n = accepted_clone.fetch_add(1, Ordering::SeqCst) + 1;
			if n <= fail_count {
				// Drop immediately — connection RST without an HTTP response.
				drop(sock);
				continue;
			}
			tokio::spawn(async move {
				let io = TokioIo::new(sock);
				let _ = hyper::server::conn::http1::Builder::new()
					.serve_connection(io, service_fn(serve_ok))
					.await;
			});
		}
	});
	(addr, accepted)
}

async fn serve_ok(
	_req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
	Ok(
		hyper::Response::builder()
			.status(200)
			.body(Full::<Bytes>::new(Bytes::from_static(b"ok")))
			.expect("build resp"),
	)
}

#[tokio::test]
async fn retry_recovers_from_transient_unreachable_when_max_attempts_3() {
	vane_engine::crypto::install_default_provider();
	let (upstream, accepted) = spawn_flaky_upstream(2).await;
	let proxy_addr = pick_port().await;
	// `force` buffering is required for retry against an inbound
	// `Body::Stream` (the listener always wraps incoming H1 bodies
	// in `IncomingAdapter`). The lower-pass equivalent is the
	// `retry_buffer_required: true` flag — set both here so the
	// runtime sees `Body::Static` after pre-collection.
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.to_string(),
			"version": "h1",
			"retry": { "max_attempts": 3, "backoff": "none", "buffering": "force" },
		}),
		true,
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let (status, body) = h1_send(proxy_addr, "GET", None).await;
	let n = accepted.load(Ordering::SeqCst);
	assert_eq!(status, 200, "third attempt must succeed (accepts={n})");
	assert_eq!(body.as_ref(), b"ok");
	assert_eq!(n, 3, "exactly 3 upstream accepts: 2 refused + 1 served");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn retry_does_not_fire_for_post_unless_explicitly_opted_in() {
	vane_engine::crypto::install_default_provider();
	let (upstream, accepted) = spawn_flaky_upstream(usize::MAX).await; // every accept refuses
	let proxy_addr = pick_port().await;
	// Default methods exclude POST → max_attempts collapses to 1.
	// `force` + `retry_buffer_required: true` so the body shape
	// doesn't itself disable retry (we want method gating to be the
	// load-bearing assertion).
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.to_string(),
			"version": "h1",
			"retry": { "max_attempts": 5, "backoff": "none", "buffering": "force" },
		}),
		true,
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let _ = h1_send(proxy_addr, "POST", Some(Bytes::from_static(b"payload"))).await;
	tokio::time::sleep(Duration::from_millis(30)).await;
	let n = accepted.load(Ordering::SeqCst);
	assert_eq!(n, 1, "POST without explicit opt-in must not retry; got {n} accepts");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn retry_fires_for_post_when_explicitly_opted_in() {
	vane_engine::crypto::install_default_provider();
	let (upstream, accepted) = spawn_flaky_upstream(2).await;
	let proxy_addr = pick_port().await;
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.to_string(),
			"version": "h1",
			"retry": {
				"max_attempts": 3,
				"methods": ["POST", "GET"],
				"backoff": "none",
				"buffering": "force",
			},
		}),
		true,
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let (status, _) = h1_send(proxy_addr, "POST", Some(Bytes::from_static(b"payload"))).await;
	assert_eq!(status, 200);
	let n = accepted.load(Ordering::SeqCst);
	assert_eq!(n, 3, "explicit POST opt-in retries until success; got {n} accepts");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn retry_emits_backoff_delay_between_attempts() {
	vane_engine::crypto::install_default_provider();
	let (upstream, _accepted) = spawn_flaky_upstream(2).await;
	let proxy_addr = pick_port().await;
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.to_string(),
			"version": "h1",
			"retry": {
				"max_attempts": 3,
				"backoff": { "fixed": "200ms" },
				"buffering": "force",
			},
		}),
		true,
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let started = Instant::now();
	let (status, _) = h1_send(proxy_addr, "GET", None).await;
	let elapsed = started.elapsed();
	assert_eq!(status, 200);
	assert!(
		elapsed >= Duration::from_millis(400),
		"two retries with 200ms fixed backoff must accumulate at least 400ms — got {elapsed:?}",
	);

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn retry_does_not_fire_on_non_retryable_response() {
	vane_engine::crypto::install_default_provider();
	// Upstream that always serves a 4xx — `Error::is_retryable()`
	// considers a clean HTTP exchange (regardless of status) a
	// success at the fetch layer; the proxy returns the response
	// without retrying.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let upstream = listener.local_addr().expect("local_addr");
	let accepted = Arc::new(AtomicUsize::new(0));
	let accepted_clone = Arc::clone(&accepted);
	tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			accepted_clone.fetch_add(1, Ordering::SeqCst);
			tokio::spawn(async move {
				let io = TokioIo::new(sock);
				let svc = service_fn(|_req: hyper::Request<hyper::body::Incoming>| async {
					Ok::<_, Infallible>(
						hyper::Response::builder()
							.status(404)
							.body(Full::<Bytes>::new(Bytes::from_static(b"nope")))
							.expect("build resp"),
					)
				});
				let _ = hyper::server::conn::http1::Builder::new().serve_connection(io, svc).await;
			});
		}
	});

	let proxy_addr = pick_port().await;
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.to_string(),
			"version": "h1",
			"retry": { "max_attempts": 5, "backoff": "none" },
		}),
		false,
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let (status, _) = h1_send(proxy_addr, "GET", None).await;
	assert_eq!(status, 404, "404 from upstream surfaces verbatim");
	tokio::time::sleep(Duration::from_millis(50)).await;
	let n = accepted.load(Ordering::SeqCst);
	assert_eq!(n, 1, "non-retryable response must not retry");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn retry_does_not_fire_for_streaming_request_body_under_opportunistic() {
	vane_engine::crypto::install_default_provider();
	let (upstream, accepted) = spawn_flaky_upstream(usize::MAX).await;
	let proxy_addr = pick_port().await;
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.to_string(),
			"version": "h1",
			"retry": {
				"max_attempts": 3,
				"methods": ["POST", "GET"],
				"backoff": "none",
				"buffering": "opportunistic",
			},
		}),
		false, // lower didn't flag → fetch sees Body::Stream
	);
	let (set, proxy_addr) = start_listener(graph).await;

	// Hyper H1 client with `Empty<Bytes>` body sends Content-Length: 0
	// and the engine wraps the inbound body as `Body::Stream` via
	// `IncomingAdapter` (the listener's request-side adapter). With
	// opportunistic buffering, the streaming body collapses retry to
	// a single attempt.
	let _ = h1_send(proxy_addr, "POST", None).await;
	tokio::time::sleep(Duration::from_millis(30)).await;
	let n = accepted.load(Ordering::SeqCst);
	assert_eq!(n, 1, "opportunistic + streaming body must single-attempt; got {n} accepts");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn retry_fires_for_streaming_request_body_under_force() {
	vane_engine::crypto::install_default_provider();
	let (upstream, accepted) = spawn_flaky_upstream(2).await;
	let proxy_addr = pick_port().await;
	// `retry_buffer_required: true` simulates the lower-pass output
	// for a `force`-buffering policy: the executor pre-collects the
	// request body before the fetch runs, so the fetch sees
	// `Body::Static`.
	let graph = proxy_graph(
		proxy_addr,
		serde_json::json!({
			"upstream": upstream.to_string(),
			"version": "h1",
			"retry": {
				"max_attempts": 3,
				"methods": ["POST", "GET"],
				"backoff": "none",
				"buffering": "force",
			},
		}),
		true,
	);
	let (set, proxy_addr) = start_listener(graph).await;

	let (status, _) = h1_send(proxy_addr, "POST", Some(Bytes::from_static(b"payload"))).await;
	assert_eq!(status, 200, "force buffering buffers the body so retry survives transient failures");
	let n = accepted.load(Ordering::SeqCst);
	assert_eq!(n, 3, "force + transient refusals retry to success; got {n} accepts");

	set.shutdown(Duration::from_millis(500)).await;
}
