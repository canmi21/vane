//! Integration tests for `vane_engine::fetch::http_synthesize`.
//!
//! Covers the in-memory response Fetch contract described in
//! `spec/architecture/05-terminator.md` § _`HttpSynthesize`_,
//! `spec/architecture/07-l7.md` § _`HttpProxyFetch` commits to streaming
//! response bodies_ ("`HttpSynthesizeFetch` always produces `Body::Static`
//! by construction — that is the point of synthesis"), and
//! `spec/architecture/14-presets.md` § _`static_site`_ for the args shape:
//!
//! ```json
//! {
//!   "status":  200,
//!   "headers": { "content-type": "text/plain" },
//!   "body":    "<base64 of raw bytes>"
//! }
//! ```
//!
//! End-to-end tests build a `SymbolicFlowGraph` rooted at
//! `Upgrade -> Fetch(HttpSynthesize) -> Terminate(WriteHttpResponse)`,
//! drive a hyper H1 client at the listener, and assert the wire-level
//! response. Factory-arg validation tests call `factory(args)` directly.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, Node, NodeId, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FactoryError, FetchFactories, MiddlewareFactories};
use vane_engine::fetch::http_synthesize::{
	factory as http_synth_factory, register as register_http_synth,
};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

// ---------------------------------------------------------------------------
// FlowLogSink fixture: drops events.
// ---------------------------------------------------------------------------

struct DropSink;

impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

// ---------------------------------------------------------------------------
// Free-port discovery — bind ephemeral, take `local_addr()`, drop.
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
// Build the standard graph for synthesis tests:
//
//   entry(Upgrade { next: 1 })
//     -> Fetch { id: 0, kind = HttpSynthesize, args, next_response: 2 }
//       -> Terminate(WriteHttpResponse)
//
// Per `02-flow.md` § _Phase state machine_, Upgrade transitions
// L4Raw → L7Request, satisfying the Fetch node's phase precondition.
// ---------------------------------------------------------------------------

fn synth_graph(listen: SocketAddr, args: serde_json::Value) -> Arc<FlowGraph> {
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
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args }],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta: sample_meta(),
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	register_http_synth(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link http_synthesize graph")
}

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("graph has at least one entry").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(graph, verbosity, sink);
	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

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

/// Standard base64 encode for the args shape — `HttpSynthesizeFetch` reads
/// the body field as base64 per 14-presets.md / the public factory
/// docstring (the preset expansion stage is responsible for translating
/// user-friendly text into base64 before this factory).
fn b64(bytes: &[u8]) -> String {
	use base64::Engine as _;
	base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---------------------------------------------------------------------------
// 8. http_synthesize_returns_static_response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_synthesize_returns_static_response() {
	// 05-terminator.md § _`HttpSynthesize`_ + 14-presets.md § _`static_site`_:
	// the factory's `status` and `body` parameterise the synthesised
	// response. The body is base64-encoded raw bytes — the preset
	// expansion stage converts user-friendly text upstream of this
	// factory. End-to-end the client sees the decoded bytes verbatim.
	let proxy_addr = pick_port().await;
	let args = serde_json::json!({ "status": 200, "body": b64(b"hello") });
	let graph = synth_graph(proxy_addr, args);
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET request");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "synthesised status must surface verbatim");
	let body = resp.into_body().collect().await.expect("collect body").to_bytes();
	assert_eq!(body.as_ref(), b"hello", "synthesised body bytes must match the decoded args.body");

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_secs(2)).await;
}

// ---------------------------------------------------------------------------
// 9. http_synthesize_with_headers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_synthesize_with_headers() {
	// 05-terminator.md § _`HttpSynthesize`_: synthesised responses carry
	// configured headers verbatim. Both an arbitrary custom header and
	// `content-type` must reach the client.
	let proxy_addr = pick_port().await;
	let args = serde_json::json!({
		"status": 200,
		"headers": { "x-via": "vane", "content-type": "text/plain" },
	});
	let graph = synth_graph(proxy_addr, args);
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET request");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "synthesised status must surface verbatim");
	assert_eq!(
		resp.headers().get("x-via").and_then(|v| v.to_str().ok()),
		Some("vane"),
		"client must receive the configured X-Via header verbatim",
	);
	assert_eq!(
		resp.headers().get("content-type").and_then(|v| v.to_str().ok()),
		Some("text/plain"),
		"client must receive the configured Content-Type header verbatim",
	);

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_secs(2)).await;
}

// ---------------------------------------------------------------------------
// 10. http_synthesize_empty_body_writes_empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_synthesize_empty_body_writes_empty() {
	// 07-l7.md § _`HttpProxyFetch` commits to streaming response bodies_:
	// "`HttpSynthesizeFetch` always produces `Body::Static` by
	// construction"; an empty body is the equivalent zero-frame case.
	// 204 (No Content) is a valid HTTP status and the natural status code
	// to pair with an empty body. The client sees a 204 with zero bytes.
	let proxy_addr = pick_port().await;
	let args = serde_json::json!({ "status": 204 });
	let graph = synth_graph(proxy_addr, args);
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET request");

	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 204, "synthesised 204 must surface verbatim");
	let body = resp.into_body().collect().await.expect("collect body").to_bytes();
	assert_eq!(body.len(), 0, "204 with no body arg must write zero bytes to the wire");

	tokio::task::yield_now().await;
	set.shutdown(Duration::from_secs(2)).await;
}

// ---------------------------------------------------------------------------
// 11. http_synthesize_factory_rejects_invalid_status
// ---------------------------------------------------------------------------

#[test]
fn http_synthesize_factory_rejects_invalid_status() {
	// Per the public docstring on `http_synthesize::factory`: `status`
	// must be an integer in the HTTP range `100..=599`. Each negative
	// case below — out-of-range low, out-of-range high, wrong type —
	// must return `Err(FactoryError(_))`. `FetchInst` does not implement
	// `Debug`, so let-else is the destructure strategy.
	let Err(FactoryError(_)) = http_synth_factory(&serde_json::json!({ "status": 99 })) else {
		panic!("status 99 must be rejected as out-of-range");
	};
	let Err(FactoryError(_)) = http_synth_factory(&serde_json::json!({ "status": 600 })) else {
		panic!("status 600 must be rejected as out-of-range");
	};
	let Err(FactoryError(_)) = http_synth_factory(&serde_json::json!({ "status": "200" })) else {
		panic!("status as string must be rejected — args.status is an integer");
	};
}

// ---------------------------------------------------------------------------
// 12. http_synthesize_factory_rejects_invalid_header_name
// ---------------------------------------------------------------------------

#[test]
fn http_synthesize_factory_rejects_invalid_header_name() {
	// HTTP header names follow RFC 7230 token grammar — spaces are not
	// permitted. The factory pre-validates names as `http::HeaderName`
	// per its docstring, so an invalid name must surface at link time
	// rather than at request time.
	let args = serde_json::json!({
		"status": 200,
		"headers": { "bad name": "x" },
	});
	let Err(FactoryError(msg)) = http_synth_factory(&args) else {
		panic!("invalid header name must be rejected; got Ok(_)");
	};
	assert!(
		msg.to_lowercase().contains("header") || msg.to_lowercase().contains("name"),
		"FactoryError message must explain the invalid header name; got {msg:?}",
	);
}

// ---------------------------------------------------------------------------
// 13. http_synthesize_factory_rejects_non_string_header_value
// ---------------------------------------------------------------------------

#[test]
fn http_synthesize_factory_rejects_non_string_header_value() {
	// Per the factory's public docstring: header values must be strings.
	// JSON has no native byte type and the synth path does not encode
	// integer values; a non-string value is a config-time error.
	let args = serde_json::json!({
		"status": 200,
		"headers": { "x": 42 },
	});
	let Err(FactoryError(_)) = http_synth_factory(&args) else {
		panic!("non-string header value must be rejected; got Ok(_)");
	};
}

// ---------------------------------------------------------------------------
// 14. http_synthesize_factory_rejects_invalid_base64_body
// ---------------------------------------------------------------------------

#[test]
fn http_synthesize_factory_rejects_invalid_base64_body() {
	// Per the factory's public docstring: `body` is base64-encoded raw
	// bytes. `"!!!"` is not valid base64; the factory must surface the
	// rejection rather than silently treating the bytes as data.
	let args = serde_json::json!({ "status": 200, "body": "!!!" });
	let Err(FactoryError(msg)) = http_synth_factory(&args) else {
		panic!("invalid base64 body must be rejected; got Ok(_)");
	};
	assert!(
		msg.to_lowercase().contains("base64"),
		"FactoryError message must reference base64; got {msg:?}",
	);
}
