//! Integration tests for `vane_engine::middleware::host_header_match`.
//!
//! Validates the public contract from `spec/architecture/04-middleware.md`
//! § _Stateless internal_ and the doc-comment on
//! `host_header_match::factory`:
//!
//! - Continue when the request `Host` header (case-insensitive) matches one
//!   of the configured authorities.
//! - Short-circuit-close otherwise (executor surfaces this as Err).
//! - Factory rejects empty / missing / non-string `hosts` configuration.
//!
//! Treats the middleware as a black box — drives it through `execute` with
//! a 3-node graph `Middleware(L7Request) -> Fetch(L7) -> Terminate(...)`.

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
use vane_engine::executor::{ExecutorInput, execute};
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FetchInst, FlowGraph};
use vane_engine::middleware::host_header_match;

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

/// Counts fetch invocations so a missed short-close (i.e. middleware
/// erroneously continued) is observable as `count > 0`.
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

/// Builds the 3-node graph: Middleware(L7Request) -> Fetch(L7) ->
/// Terminate(WriteHttpResponse). Returns the linked graph and the fetch
/// counter so the caller can read whether the fetch actually ran.
fn link_graph(host_args: Value) -> (Arc<FlowGraph>, Arc<AtomicUsize>) {
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
			},
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![l7_req_ref_with_args("host_header_match", host_args)],
		vec![SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: Value::Null }],
		vec![Terminator::WriteHttpResponse],
	);
	let mut mw = MiddlewareFactories::new();
	host_header_match::register(&mut mw);
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

fn link_graph_expect_err(host_args: Value) -> String {
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
			},
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![l7_req_ref_with_args("host_header_match", host_args)],
		vec![SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: Value::Null }],
		vec![Terminator::WriteHttpResponse],
	);
	let mut mw = MiddlewareFactories::new();
	host_header_match::register(&mut mw);
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

fn req_with_host(host: Option<&str>) -> Request {
	let mut b = http::Request::builder().method("GET").uri("/");
	if let Some(h) = host {
		b = b.header("host", h);
	}
	b.body(Body::Empty).expect("build req")
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn host_match_continues_when_host_in_list() {
	let (graph, counter) = link_graph(json!({ "hosts": ["api.example.com"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_host(Some("api.example.com")))),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_ok(), "match must continue to fetch+terminate; got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 1, "fetch must run on a host match");
}

#[tokio::test]
async fn host_match_case_insensitive_match() {
	// Factory pre-lowercases the configured list; runtime lowercases the
	// incoming header. Mixed-case configuration vs lower-case request
	// should still match.
	let (graph, counter) = link_graph(json!({ "hosts": ["API.Example.COM"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_host(Some("api.example.com")))),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_ok(), "case-insensitive host match must continue; got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 1, "fetch must run on a case-insensitive match");
}

#[tokio::test]
async fn host_match_short_close_when_host_missing() {
	// FIXME(executor-short-close-routing): client should see 404 once
	// executor refines Short(Close) routing; for now the executor surfaces
	// short-close as Err.
	let (graph, counter) = link_graph(json!({ "hosts": ["api.example.com"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_host(None))),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_err(), "missing Host header must short-close; got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 0, "fetch must not run when middleware short-closes");
	let kinds = sink.kinds();
	assert!(
		kinds.contains(&FlowLogKind::Trajectory),
		"short-close still emits a Trajectory event; got {kinds:?}",
	);
}

#[tokio::test]
async fn host_match_short_close_when_no_host_matches() {
	// FIXME(executor-short-close-routing): client should see 404 once
	// executor refines Short(Close) routing.
	let (graph, counter) = link_graph(json!({ "hosts": ["api.example.com"] }));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(req_with_host(Some("other.com")))),
		&conn,
		&sink,
	)
	.await;
	assert!(result.is_err(), "non-matching Host must short-close; got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 0, "fetch must not run on host miss");
	let kinds = sink.kinds();
	assert!(
		kinds.contains(&FlowLogKind::Trajectory),
		"short-close still emits a Trajectory event; got {kinds:?}",
	);
}

#[tokio::test]
async fn host_match_factory_rejects_empty_hosts_array() {
	let rendered = link_graph_expect_err(json!({ "hosts": [] }));
	assert!(
		rendered.contains("at least one"),
		"empty hosts must be rejected with an 'at least one' message; got {rendered:?}",
	);
}

#[tokio::test]
async fn host_match_factory_rejects_missing_hosts() {
	let rendered = link_graph_expect_err(json!({}));
	assert!(
		rendered.to_lowercase().contains("hosts"),
		"missing hosts must surface a hosts-shaped error; got {rendered:?}",
	);
}

#[tokio::test]
async fn host_match_factory_rejects_non_string_element() {
	let rendered = link_graph_expect_err(json!({ "hosts": [42] }));
	assert!(
		rendered.to_lowercase().contains("string"),
		"non-string element must be rejected with a string-shaped error; got {rendered:?}",
	);
}
