//! Integration tests for `vane_engine::middleware::forward_client_ip`.
//!
//! Validates the public contract from the module-level doc-comment:
//!
//! - `X-Forwarded-For` is appended (existing chain preserved with a `, `
//!   separator).
//! - `X-Real-IP` is overwritten (the L4 peer is authoritative).
//! - Default header set is both; `headers` arg accepts a subset.
//! - Always returns `Decision::Continue` — never short-circuits.
//! - Factory rejects unsupported header names with the offending name in
//!   the error message.
//!
//! Captures the post-middleware request headers via a `CaptureHeadersFetch`
//! installed under `FetchKind::HttpSynthesize` so the main thread can
//! inspect what the middleware actually wrote.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use vane_core::{
	Body, ConnContext, ConnId, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, FlowLogEvent,
	FlowLogSink, L7Fetch, L7FetchOutput, MiddlewareId, MiddlewareKind, Node, NodeId, Request,
	Response, SymbolicFetchRef, SymbolicFlowGraph, SymbolicMiddlewareRef, Terminator, TerminatorId,
	Transport,
};
use vane_engine::executor::{ExecutorInput, execute};
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FetchInst, FlowGraph};
use vane_engine::middleware::forward_client_ip;

// Sink + conn / graph fixtures (copied from tests/executor.rs).

struct NullSink {
	events: Mutex<Vec<FlowLogEvent>>,
}

impl NullSink {
	fn new() -> Self {
		Self { events: Mutex::new(Vec::new()) }
	}
}

impl FlowLogSink for NullSink {
	fn emit(&self, event: FlowLogEvent) {
		self.events.lock().push(event);
	}
}

fn make_conn(remote: &str) -> Arc<ConnContext> {
	let remote: SocketAddr = remote.parse().expect("parse remote addr");
	let local: SocketAddr = "127.0.0.1:0".parse().expect("parse local addr");
	Arc::new(ConnContext::new(ConnId(1), remote, local, Transport::Tcp, Instant::now()))
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

		listener_transports: std::collections::BTreeMap::new(),
		annotations: Vec::new(),
	}
}

fn build_graph(
	nodes: Vec<Node>,
	middlewares: Vec<SymbolicMiddlewareRef>,
	fetches: Vec<SymbolicFetchRef>,
	terminators: Vec<Terminator>,
) -> Arc<SymbolicFlowGraph> {
	Arc::new(SymbolicFlowGraph {
		nodes,
		predicates: vec![],
		middlewares,
		fetches,
		terminators,
		entries: HashMap::new(),
		meta: sample_meta(),
	})
}

fn l7_req_ref_with_args(name: &str, args: Value) -> SymbolicMiddlewareRef {
	SymbolicMiddlewareRef {
		name: Arc::from(name),
		args,
		kind: MiddlewareKind::L7Request,
		stateless: true,
		needs_body: false,
		on_error: None,
	}
}

/// Stores the post-middleware request headers so the main thread can
/// inspect what `forward_client_ip` actually wrote.
struct CaptureHeadersFetch {
	captured: Arc<Mutex<Option<http::HeaderMap>>>,
}

#[async_trait]
impl L7Fetch for CaptureHeadersFetch {
	async fn fetch(
		&self,
		req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		*self.captured.lock() = Some(req.headers().clone());
		let resp: Response =
			http::Response::builder().status(200).body(Body::Empty).expect("build resp");
		Ok(L7FetchOutput::Response(resp))
	}
}

async fn run_execute(
	graph: &Arc<FlowGraph>,
	entry: NodeId,
	input: ExecutorInput,
	conn: &Arc<ConnContext>,
	sink: &Arc<NullSink>,
) -> Result<vane_engine::executor::ExecutorOutput, Error> {
	let mut ctx = FlowCtx {
		span: tracing::Span::none(),
		log: Arc::clone(sink) as Arc<dyn FlowLogSink>,
		cancel: CancellationToken::new(),
		accept_cancel: CancellationToken::new(),
		verbosity: vane_core::FlowLogVerbosity::Trajectory,
		trajectory: vane_core::TrajectoryBuilder::new(conn.id, entry, 0),
	};
	execute(graph, entry, input, conn, &mut ctx).await
}

/// Build a 3-node graph driven by `forward_client_ip` with the given
/// args, terminating in a `CaptureHeadersFetch` so tests can inspect the
/// post-middleware request headers.
fn link_graph(fwd_args: Value) -> (Arc<FlowGraph>, Arc<Mutex<Option<http::HeaderMap>>>) {
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::for_testing(0),
				next: NodeId::for_testing(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Fetch {
				id: FetchId::for_testing(0),
				next_response: Some(NodeId::for_testing(2)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::for_testing(0)),
		],
		vec![l7_req_ref_with_args("forward_client_ip", fwd_args)],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		vec![Terminator::WriteHttpResponse],
	);
	let mut mw = MiddlewareFactories::new();
	forward_client_ip::register(&mut mw);
	let captured: Arc<Mutex<Option<http::HeaderMap>>> = Arc::new(Mutex::new(None));
	let mut fetch = FetchFactories::new();
	{
		let captured = Arc::clone(&captured);
		fetch.register(FetchKind::HttpSynthesize, move |_args| {
			Ok(FetchInst::L7(Arc::new(CaptureHeadersFetch { captured: Arc::clone(&captured) })))
		});
	}
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	(graph, captured)
}

fn link_graph_expect_err(fwd_args: Value) -> String {
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::for_testing(0),
				next: NodeId::for_testing(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Fetch {
				id: FetchId::for_testing(0),
				next_response: Some(NodeId::for_testing(2)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::for_testing(0)),
		],
		vec![l7_req_ref_with_args("forward_client_ip", fwd_args)],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		vec![Terminator::WriteHttpResponse],
	);
	let mut mw = MiddlewareFactories::new();
	forward_client_ip::register(&mut mw);
	let captured: Arc<Mutex<Option<http::HeaderMap>>> = Arc::new(Mutex::new(None));
	let mut fetch = FetchFactories::new();
	{
		let captured = Arc::clone(&captured);
		fetch.register(FetchKind::HttpSynthesize, move |_args| {
			Ok(FetchInst::L7(Arc::new(CaptureHeadersFetch { captured: Arc::clone(&captured) })))
		});
	}
	match FlowGraph::link(sym, &mw, &fetch) {
		Ok(_) => panic!("link should reject these args, but it succeeded"),
		Err(e) => e.to_string(),
	}
}

fn empty_get() -> Request {
	http::Request::builder().method("GET").uri("/").body(Body::Empty).expect("build req")
}

fn req_with_headers(headers: &[(&str, &str)]) -> Request {
	let mut b = http::Request::builder().method("GET").uri("/");
	for (k, v) in headers {
		b = b.header(*k, *v);
	}
	b.body(Body::Empty).expect("build req")
}

// Tests.

#[tokio::test]
async fn forward_client_ip_default_adds_both_headers() {
	let (graph, captured) = link_graph(Value::Null);
	let conn = make_conn("127.0.0.1:54321");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::for_testing(0),
		ExecutorInput::L7(Box::new(empty_get())),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_ok(), "forward_client_ip must continue; got {result:?}");
	let headers = captured.lock().clone().expect("downstream fetch must have captured headers");
	assert_eq!(
		headers.get("x-forwarded-for").map(|v| v.to_str().unwrap()),
		Some("127.0.0.1"),
		"default config must set X-Forwarded-For; headers={headers:?}",
	);
	assert_eq!(
		headers.get("x-real-ip").map(|v| v.to_str().unwrap()),
		Some("127.0.0.1"),
		"default config must set X-Real-IP; headers={headers:?}",
	);
}

#[tokio::test]
async fn forward_client_ip_xff_appends_to_existing() {
	// Per doc-comment: "If the request already carries one, the client IP
	// is appended after a `, ` separator so the chain is preserved
	// (`upstream-proxy.ip, our-client.ip`)."
	let (graph, captured) = link_graph(Value::Null);
	let conn = make_conn("127.0.0.1:54321");
	let sink = Arc::new(NullSink::new());
	let req = req_with_headers(&[("X-Forwarded-For", "1.2.3.4")]);
	let result =
		run_execute(&graph, NodeId::for_testing(0), ExecutorInput::L7(Box::new(req)), &conn, &sink)
			.await;
	assert!(result.is_ok(), "forward_client_ip must continue; got {result:?}");
	let headers = captured.lock().clone().expect("captured headers");
	let xff = headers.get("x-forwarded-for").expect("XFF must be present").to_str().unwrap();
	assert_eq!(xff, "1.2.3.4, 127.0.0.1", "XFF must be appended preserving the chain; got {xff:?}");
}

#[tokio::test]
async fn forward_client_ip_real_ip_overwrites_existing() {
	// Per doc-comment: "X-Real-IP — overwrite. Always set to the L4 peer;
	// an upstream proxy's claim is intentionally clobbered."
	let (graph, captured) = link_graph(Value::Null);
	let conn = make_conn("127.0.0.1:54321");
	let sink = Arc::new(NullSink::new());
	let req = req_with_headers(&[("X-Real-IP", "8.8.8.8")]);
	let result =
		run_execute(&graph, NodeId::for_testing(0), ExecutorInput::L7(Box::new(req)), &conn, &sink)
			.await;
	assert!(result.is_ok(), "forward_client_ip must continue; got {result:?}");
	let headers = captured.lock().clone().expect("captured headers");
	let real_ip = headers.get("x-real-ip").expect("X-Real-IP must be present").to_str().unwrap();
	assert_eq!(
		real_ip, "127.0.0.1",
		"X-Real-IP must be overwritten with the L4 peer; got {real_ip:?}"
	);
	// Sanity: there should be exactly one X-Real-IP value, not two.
	let count = headers.get_all("x-real-ip").iter().count();
	assert_eq!(count, 1, "X-Real-IP must collapse to a single value; got {count}");
}

#[tokio::test]
async fn forward_client_ip_subset_only_xff() {
	// Per doc-comment: `headers` accepts a subset of the supported names.
	let (graph, captured) = link_graph(json!({ "headers": ["x-forwarded-for"] }));
	let conn = make_conn("127.0.0.1:54321");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::for_testing(0),
		ExecutorInput::L7(Box::new(empty_get())),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_ok(), "subset config must still continue; got {result:?}");
	let headers = captured.lock().clone().expect("captured headers");
	assert!(headers.get("x-forwarded-for").is_some(), "XFF must be present in subset config");
	assert!(
		headers.get("x-real-ip").is_none(),
		"X-Real-IP must be absent when subset omits it; headers={headers:?}",
	);
}

#[tokio::test]
async fn forward_client_ip_renders_ipv6_correctly() {
	// `SocketAddr::ip()` strips the brackets that `to_string()` on a
	// SocketAddr would produce; the rendered header must be just the bare
	// IPv6 literal — no zone, no brackets, no port.
	let (graph, captured) = link_graph(Value::Null);
	let conn = make_conn("[2001:db8::1]:443");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::for_testing(0),
		ExecutorInput::L7(Box::new(empty_get())),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_ok(), "forward_client_ip must continue; got {result:?}");
	let headers = captured.lock().clone().expect("captured headers");
	let xff = headers.get("x-forwarded-for").expect("XFF must be present").to_str().unwrap();
	assert_eq!(xff, "2001:db8::1", "IPv6 XFF must be bare literal (no brackets/port); got {xff:?}");
	let real_ip = headers.get("x-real-ip").expect("X-Real-IP must be present").to_str().unwrap();
	assert_eq!(real_ip, "2001:db8::1", "IPv6 X-Real-IP must be bare literal; got {real_ip:?}");
}

#[tokio::test]
async fn forward_client_ip_factory_rejects_unsupported_header_name() {
	// Per doc-comment: "anything else is rejected with a pointed error so a
	// typo (`"x-forwared-for"`) doesn't silently disable the injection."
	let rendered = link_graph_expect_err(json!({ "headers": ["x-custom"] }));
	assert!(
		rendered.contains("x-custom"),
		"unsupported header error must surface the offending name; got {rendered:?}",
	);
}

#[tokio::test]
async fn forward_client_ip_always_continues() {
	// Doc-comment: "Always returns `Decision::Continue` — this middleware
	// never short-circuits." Verified by checking the downstream fetch was
	// invoked even on an unusual request (POST with a deep path).
	let (graph, captured) = link_graph(json!({}));
	let conn = make_conn("127.0.0.1:54321");
	let sink = Arc::new(NullSink::new());
	let req = http::Request::builder()
		.method("POST")
		.uri("/very/strange/path?weird=true")
		.body(Body::Empty)
		.expect("build req");
	let result =
		run_execute(&graph, NodeId::for_testing(0), ExecutorInput::L7(Box::new(req)), &conn, &sink)
			.await;
	assert!(
		result.is_ok(),
		"forward_client_ip must continue regardless of method/path; got {result:?}"
	);
	assert!(captured.lock().is_some(), "downstream fetch must run — middleware never short-circuits");
}
