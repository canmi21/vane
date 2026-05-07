//! Integration tests for `vane_engine::middleware::path_prefix`.
//!
//! Validates the public contract from `spec/architecture/04-middleware.md`
//! § _Stateless internal_ and the doc-comment on `path_prefix::factory`:
//!
//! - Continue when the request's URI path starts with one of the
//!   configured prefixes (case-sensitive byte prefix match).
//! - Short-circuit-close otherwise.
//! - Factory rejects empty / missing / non-string `prefixes`
//!   configuration.
//!
//! Treats the middleware as a black box — drives it through `execute`
//! with a 3-node graph `Middleware(L7Request) -> Fetch(L7) ->
//! Terminate(WriteHttpResponse)`.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use vane_core::{
	Body, ConnContext, ConnId, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, FlowLogEvent,
	FlowLogKind, FlowLogSink, L7Fetch, L7FetchOutput, MiddlewareId, MiddlewareKind, Node, NodeId,
	Request, Response, SymbolicFetchRef, SymbolicFlowGraph, SymbolicMiddlewareRef, Terminator,
	TerminatorId, Transport,
};
use vane_engine::executor::{ExecutorInput, ExecutorOutput, execute};
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FetchInst, FlowGraph};
use vane_engine::middleware::path_prefix;

// ---------------------------------------------------------------------------
// Sink + conn / graph fixtures (copied from tests/executor.rs).
// ---------------------------------------------------------------------------

struct NullSink {
	events: Mutex<Vec<FlowLogEvent>>,
}

impl NullSink {
	fn new() -> Self {
		Self { events: Mutex::new(Vec::new()) }
	}

	fn kinds(&self) -> Vec<FlowLogKind> {
		self.events.lock().iter().map(|e| e.kind).collect()
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
	Arc::new(ConnContext {
		id: ConnId(1),
		remote,
		local,
		transport: Transport::Tcp,
		entered_at: Instant::now(),
		tls: Mutex::new(None),
		http_version: OnceLock::new(),
		user: Mutex::new(http::Extensions::new()),
	})
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

struct SynthOkFetch(Arc<AtomicUsize>);

#[async_trait]
impl L7Fetch for SynthOkFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		self.0.fetch_add(1, Ordering::SeqCst);
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
		verbosity: vane_core::FlowLogVerbosity::Trajectory,
		trajectory: vane_core::TrajectoryBuilder::new(conn.id, entry, 0),
	};
	execute(graph, entry, input, conn, &mut ctx).await
}

fn link_graph(prefix_args: Value) -> (Arc<FlowGraph>, Arc<AtomicUsize>) {
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![l7_req_ref_with_args("path_prefix", prefix_args)],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		vec![Terminator::WriteHttpResponse],
	);
	let mut mw = MiddlewareFactories::new();
	path_prefix::register(&mut mw);
	let counter = Arc::new(AtomicUsize::new(0));
	let mut fetch = FetchFactories::new();
	{
		let counter = Arc::clone(&counter);
		fetch.register(FetchKind::HttpSynthesize, move |_args| {
			Ok(FetchInst::L7(Arc::new(SynthOkFetch(Arc::clone(&counter)))))
		});
	}
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	(graph, counter)
}

fn link_graph_expect_err(prefix_args: Value) -> String {
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![l7_req_ref_with_args("path_prefix", prefix_args)],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		vec![Terminator::WriteHttpResponse],
	);
	let mut mw = MiddlewareFactories::new();
	path_prefix::register(&mut mw);
	let mut fetch = FetchFactories::new();
	let counter = Arc::new(AtomicUsize::new(0));
	{
		let counter = Arc::clone(&counter);
		fetch.register(FetchKind::HttpSynthesize, move |_args| {
			Ok(FetchInst::L7(Arc::new(SynthOkFetch(Arc::clone(&counter)))))
		});
	}
	match FlowGraph::link(sym, &mw, &fetch) {
		Ok(_) => panic!("link should reject these args, but it succeeded"),
		Err(e) => e.to_string(),
	}
}

fn req_with_uri(uri: &str) -> Request {
	http::Request::builder().method("GET").uri(uri).body(Body::Empty).expect("build req")
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path_prefix_continues_when_path_starts_with_prefix() {
	let (graph, counter) = link_graph(json!({ "prefixes": ["/api"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_uri("/api/users"))),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_ok(), "matching prefix must continue; got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 1, "fetch must run on a prefix match");
}

#[tokio::test]
async fn path_prefix_continues_when_any_of_multiple_prefixes_matches() {
	let (graph, counter) = link_graph(json!({ "prefixes": ["/admin", "/api"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_uri("/api/x"))),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_ok(), "any-of match must continue; got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 1, "fetch must run on second-prefix match");
}

#[tokio::test]
async fn path_prefix_short_close_when_no_prefix_matches() {
	// Per 02-flow.md § _`Terminator::Close` at L4 vs inside an HTTP
	// server_, a `Short(Close(PolicyDenied))` routing-level refusal flows
	// back as `Ok(ExecutorOutput::Closed)`. The H1 service-fn maps that
	// to 404 + `Connection: close` for the wire client.
	let (graph, counter) = link_graph(json!({ "prefixes": ["/api"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_uri("/other"))),
		&conn,
		&sink,
	)
	.await;
	assert!(
		matches!(result, Ok(ExecutorOutput::Closed)),
		"non-matching path must surface as Ok(Closed); got {result:?}",
	);
	assert_eq!(counter.load(Ordering::SeqCst), 0, "fetch must not run on a prefix miss");
	let kinds = sink.kinds();
	assert!(
		kinds.contains(&FlowLogKind::Trajectory),
		"short-close still emits a Trajectory event; got {kinds:?}",
	);
	assert!(
		kinds.contains(&FlowLogKind::Terminate),
		"PolicyDenied path must emit a Terminate milestone; got {kinds:?}",
	);
}

#[tokio::test]
async fn path_prefix_case_sensitive_no_match() {
	// Doc-comment is explicit: path comparison is case-sensitive (RFC
	// 3986 path is case-sensitive unlike scheme/authority). Miss flows
	// back as `Ok(ExecutorOutput::Closed)` per the routing-refusal
	// contract.
	let (graph, counter) = link_graph(json!({ "prefixes": ["/API"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_uri("/api"))),
		&conn,
		&sink,
	)
	.await;
	assert!(
		matches!(result, Ok(ExecutorOutput::Closed)),
		"lower-case path must not match upper-case prefix; got {result:?}",
	);
	assert_eq!(counter.load(Ordering::SeqCst), 0, "fetch must not run on case mismatch");
}

#[tokio::test]
async fn path_prefix_root_slash_matches_everything() {
	// Doc-comment: "Use the bare '/' prefix to match every path."
	let (graph, counter) = link_graph(json!({ "prefixes": ["/"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_uri("/anything/at/all"))),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_ok(), "'/' must match any path; got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 1, "fetch must run");
}

#[tokio::test]
async fn path_prefix_factory_rejects_empty_prefixes_array() {
	let rendered = link_graph_expect_err(json!({ "prefixes": [] }));
	assert!(
		rendered.to_lowercase().contains("prefixes"),
		"empty prefixes must surface a prefixes-shaped error; got {rendered:?}",
	);
}
