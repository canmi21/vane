//! Integration tests for `vane_engine::executor::execute`.
//!
//! Covers the execution-model contract described in
//! `spec/architecture/02-flow.md` § _Execution model_ (lines 330-469), the
//! middleware two-channel routing described in
//! `spec/architecture/04-middleware.md` § _Decision_ / _Two error channels,
//! not one_, and the three Terminator variants in
//! `spec/architecture/05-terminator.md`.
//!
//! Each test hand-builds a minimal `SymbolicFlowGraph`, routes it through
//! `FlowGraph::link`, and drives `execute` against it — no configuration
//! pipeline is exercised.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_util::sync::CancellationToken;
use vane_core::{
	AsyncReadWrite, Body, CloseReason, CompiledOperator, CompiledValue, ConnContext, ConnId,
	Decision, Error, FetchId, FetchKind, FieldPath, FlowCtx, FlowGraphMeta, FlowLogEvent,
	FlowLogKind, FlowLogSink, FlowLogVerbosity, FlowTrajectory, L4Conn, L4Fetch, L7Fetch,
	L7FetchOutput, L7RequestMiddleware, MiddlewareId, MiddlewareKind, Node, NodeId, PredicateId,
	PredicateInst, Request, Response, ShortCircuit, SymbolicFetchRef, SymbolicFlowGraph,
	SymbolicMiddlewareRef, Terminator, TerminatorId, TerminatorOutcomeKind, TrajectoryOutcome,
	Transport, Tunnel,
};
use vane_engine::executor::{ExecutorInput, ExecutorOutput, execute};
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FetchInst, FlowGraph, MiddlewareInst};

// ---------------------------------------------------------------------------
// Fixtures: log sink + ConnContext / FlowCtx builders.
// ---------------------------------------------------------------------------

/// Records every emitted `FlowLogEvent`, preserving insertion order. Used by
/// tests to confirm the executor emits the expected `FlowLogKind` values at
/// the expected points.
struct NullSink {
	events: Mutex<Vec<FlowLogEvent>>,
}

impl NullSink {
	fn new() -> Self {
		Self { events: Mutex::new(Vec::new()) }
	}

	fn count(&self) -> usize {
		self.events.lock().len()
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

/// Build a `SymbolicFlowGraph` from the provided node slab and optional
/// slabs for middleware / fetch / predicate entries. Every test below
/// constructs a graph through this helper so the node ordering stays
/// explicit at the call site.
fn build_graph(
	nodes: Vec<Node>,
	predicates: Vec<PredicateInst>,
	middlewares: Vec<SymbolicMiddlewareRef>,
	fetches: Vec<SymbolicFetchRef>,
	terminators: Vec<Terminator>,
) -> Arc<SymbolicFlowGraph> {
	Arc::new(SymbolicFlowGraph {
		nodes,
		predicates,
		middlewares,
		fetches,
		terminators,
		entries: HashMap::new(),
		meta: sample_meta(),
	})
}

fn l7_req_ref(name: &str) -> SymbolicMiddlewareRef {
	SymbolicMiddlewareRef {
		name: Arc::from(name),
		args: Value::Null,
		kind: MiddlewareKind::L7Request,
		stateless: true,
		needs_body: false,
		on_error: None,
	}
}

fn empty_l7_request() -> Request {
	http::Request::builder().method("GET").uri("/").body(Body::Empty).expect("build req")
}

// ---------------------------------------------------------------------------
// Middleware fixtures. Each counts invocations via an `AtomicUsize` so tests
// can assert exact call counts without inspecting executor internals.
// ---------------------------------------------------------------------------

struct CountAndContinue(Arc<AtomicUsize>);

#[async_trait]
impl L7RequestMiddleware for CountAndContinue {
	async fn run(
		&self,
		_req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		self.0.fetch_add(1, Ordering::SeqCst);
		Ok(Decision::Continue)
	}
}

struct ShortClose(std::borrow::Cow<'static, str>);

#[async_trait]
impl L7RequestMiddleware for ShortClose {
	async fn run(
		&self,
		_req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		Ok(Decision::Short(ShortCircuit::Close(CloseReason::PolicyDenied(self.0.clone()))))
	}
}

struct FailMiddleware(Arc<AtomicUsize>);

#[async_trait]
impl L7RequestMiddleware for FailMiddleware {
	async fn run(
		&self,
		_req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		self.0.fetch_add(1, Ordering::SeqCst);
		Err(Error::middleware("simulated"))
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

// ---------------------------------------------------------------------------
// Helper: drive `execute` against a linked graph. Tests call this with a
// fresh sink + context per invocation.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 1. execute_middleware_continue_advances_cursor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_middleware_continue_advances_cursor() {
	// 3-node graph: Middleware(L7Request) -> Terminate(Close). The middleware
	// returns Decision::Continue; the cursor must advance to `next`.
	// Per 02-flow.md § _Execution model_ (lines 391-392):
	//   Ok(Decision::Continue) → cur = *next
	let counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![l7_req_ref("count_and_continue")],
		vec![],
		vec![Terminator::Close],
	);
	let mut mw = MiddlewareFactories::new();
	{
		let counter = Arc::clone(&counter);
		mw.register("count_and_continue", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&counter)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	assert!(result.is_ok(), "Continue decision must walk to terminator, got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 1, "middleware must be invoked exactly once");
}

// ---------------------------------------------------------------------------
// 3. execute_middleware_short_close_returns_err
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_middleware_short_close_returns_err() {
	// 04-middleware.md § _Two error channels_: Short(Close(reason)) is an
	// application-level refusal; executor surfaces it as an Err whose
	// Display contains the reason payload.
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
			},
			// `next` is structurally valid but unreachable: the Short(Close)
			// path returns before advancing.
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![l7_req_ref("short_close")],
		vec![],
		vec![Terminator::Close],
	);
	let mut mw = MiddlewareFactories::new();
	mw.register("short_close", MiddlewareKind::L7Request, |_args| {
		Ok(MiddlewareInst::L7Request(Arc::new(ShortClose(std::borrow::Cow::Borrowed(
			"denied by policy",
		)))))
	});
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	let err = result.expect_err("Short(Close) must surface as Err");
	let rendered = err.to_string();
	assert!(
		rendered.contains("denied by policy"),
		"error Display must carry the reason payload; got {rendered:?}",
	);
}

// ---------------------------------------------------------------------------
// 4. execute_middleware_err_routes_via_on_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_middleware_err_routes_via_on_error() {
	// 04-middleware.md § _Two error channels_: Err(_) with on_error=Some(t)
	// jumps to t; the Err does not propagate. Here the target is a Close
	// terminator, so execute returns Ok.
	let counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				// on_error points at the terminator at NodeId(1).
				on_error: Some(NodeId::new(1)),
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![l7_req_ref("fail")],
		vec![],
		vec![Terminator::Close],
	);
	let mut mw = MiddlewareFactories::new();
	{
		let counter = Arc::clone(&counter);
		mw.register("fail", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(FailMiddleware(Arc::clone(&counter)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	assert!(result.is_ok(), "on_error=Some must route to target; got {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 1, "failing middleware must still be invoked once");
}

// ---------------------------------------------------------------------------
// 5. execute_middleware_err_without_on_error_propagates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_middleware_err_without_on_error_propagates() {
	// Without on_error, 04-middleware.md and the S1-15 contract say the Err
	// propagates verbatim. Assert the simulated message shows in to_string().
	let counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![l7_req_ref("fail")],
		vec![],
		vec![Terminator::Close],
	);
	let mut mw = MiddlewareFactories::new();
	{
		let counter = Arc::clone(&counter);
		mw.register("fail", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(FailMiddleware(Arc::clone(&counter)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	let err = result.expect_err("Err without on_error must propagate");
	let rendered = err.to_string();
	assert!(
		rendered.contains("simulated"),
		"error text must carry the middleware's ctx ('simulated'); got {rendered:?}",
	);
}

// ---------------------------------------------------------------------------
// 6. execute_check_routes_by_predicate_remote_ip_equals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_check_routes_by_predicate_remote_ip_equals() {
	// 02-flow.md § _Execution model_: Check builds a PredicateView, tests
	// the predicate, and routes to on_match / on_miss.
	// Predicate: remote.ip == 127.0.0.1.
	// Graph:
	//   0: Check { on_match=1, on_miss=3 }
	//   1: Middleware(hit_match) -> 2
	//   2: Terminate(Close)
	//   3: Middleware(hit_miss)  -> 4
	//   4: Terminate(Close)
	let hit_match = Arc::new(AtomicUsize::new(0));
	let hit_miss = Arc::new(AtomicUsize::new(0));

	let sym = build_graph(
		vec![
			Node::Check {
				predicate: PredicateId::new(0),
				on_match: NodeId::new(1),
				on_miss: NodeId::new(3),
				collect_body_before: None,
			},
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
			Node::Middleware {
				id: MiddlewareId::new(1),
				next: NodeId::new(4),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![PredicateInst {
			path: FieldPath::RemoteIp,
			op: CompiledOperator::Equals(CompiledValue::Addr("127.0.0.1".parse().expect("parse v4"))),
		}],
		vec![l7_req_ref("hit_match"), l7_req_ref("hit_miss")],
		vec![],
		vec![Terminator::Close],
	);

	let mut mw = MiddlewareFactories::new();
	{
		let hit = Arc::clone(&hit_match);
		mw.register("hit_match", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&hit)))))
		});
	}
	{
		let hit = Arc::clone(&hit_miss);
		mw.register("hit_miss", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&hit)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");

	// Matching case: remote.ip == 127.0.0.1 → on_match branch.
	let conn_match = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let r = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn_match,
		&sink,
	)
	.await;
	assert!(r.is_ok(), "match branch must complete via Close: {r:?}");
	assert_eq!(hit_match.load(Ordering::SeqCst), 1, "match branch middleware fired once");
	assert_eq!(hit_miss.load(Ordering::SeqCst), 0, "miss branch middleware must not fire");

	// Non-matching case: remote.ip = 10.0.0.1 → on_miss branch.
	let conn_miss = make_conn("10.0.0.1:0");
	let sink = Arc::new(NullSink::new());
	let r = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn_miss,
		&sink,
	)
	.await;
	assert!(r.is_ok(), "miss branch must complete via Close: {r:?}");
	assert_eq!(hit_match.load(Ordering::SeqCst), 1, "match counter is unchanged by miss run");
	assert_eq!(hit_miss.load(Ordering::SeqCst), 1, "miss branch middleware fired once");
}

// ---------------------------------------------------------------------------
// 7. execute_check_routes_by_predicate_http_method_equals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_check_routes_by_predicate_http_method_equals() {
	// Predicate: http.method == "GET". Same graph shape as test 6, but the
	// distinguishing input is the request's method.
	let hit_match = Arc::new(AtomicUsize::new(0));
	let hit_miss = Arc::new(AtomicUsize::new(0));

	let sym = build_graph(
		vec![
			Node::Check {
				predicate: PredicateId::new(0),
				on_match: NodeId::new(1),
				on_miss: NodeId::new(3),
				collect_body_before: None,
			},
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
			Node::Middleware {
				id: MiddlewareId::new(1),
				next: NodeId::new(4),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![PredicateInst {
			path: FieldPath::HttpMethod,
			op: CompiledOperator::Equals(CompiledValue::Str(Arc::from("GET"))),
		}],
		vec![l7_req_ref("hit_match"), l7_req_ref("hit_miss")],
		vec![],
		vec![Terminator::Close],
	);

	let mut mw = MiddlewareFactories::new();
	{
		let hit = Arc::clone(&hit_match);
		mw.register("hit_match", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&hit)))))
		});
	}
	{
		let hit = Arc::clone(&hit_miss);
		mw.register("hit_miss", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&hit)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");

	let conn = make_conn("127.0.0.1:0");

	// GET → on_match.
	let get_req: Request =
		http::Request::builder().method("GET").uri("/").body(Body::Empty).expect("build GET");
	let sink = Arc::new(NullSink::new());
	let r =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(get_req)), &conn, &sink).await;
	assert!(r.is_ok(), "GET must traverse the match branch: {r:?}");
	assert_eq!(hit_match.load(Ordering::SeqCst), 1, "GET runs the match branch once");
	assert_eq!(hit_miss.load(Ordering::SeqCst), 0, "GET must not run the miss branch");

	// POST → on_miss.
	let post_req: Request =
		http::Request::builder().method("POST").uri("/").body(Body::Empty).expect("build POST");
	let sink = Arc::new(NullSink::new());
	let r =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(post_req)), &conn, &sink).await;
	assert!(r.is_ok(), "POST must traverse the miss branch: {r:?}");
	assert_eq!(hit_match.load(Ordering::SeqCst), 1, "match counter unchanged");
	assert_eq!(hit_miss.load(Ordering::SeqCst), 1, "POST runs the miss branch once");
}

// ---------------------------------------------------------------------------
// 8. execute_l7_fetch_response_jumps_to_next_response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_l7_fetch_response_jumps_to_next_response() {
	// 02-flow.md § _Execution model_ (lines 411-424): an L7 Fetch that
	// returns L7FetchOutput::Response advances the cursor to
	// `next_response`. The terminating node here is
	// Terminate(WriteHttpResponse); per the behavior contract
	// WriteHttpResponse drops the response and returns Ok(()).
	let fetch_counter = Arc::new(AtomicUsize::new(0));

	let sym = build_graph(
		vec![
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(1)),
				next_tunnel: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: Value::Null }],
		vec![Terminator::WriteHttpResponse],
	);
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	{
		let counter = Arc::clone(&fetch_counter);
		fetch.register(FetchKind::HttpSynthesize, move |_args| {
			Ok(FetchInst::L7(Arc::new(SynthOkFetch(Arc::clone(&counter)))))
		});
	}
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	assert!(result.is_ok(), "L7 Fetch→Response→WriteHttpResponse must succeed: {result:?}");
	assert_eq!(fetch_counter.load(Ordering::SeqCst), 1, "fetch factory impl must be invoked once");
}

// ---------------------------------------------------------------------------
// (Test 9 — `execute_upgrade_node_errors_as_unsupported` — was removed when
// `Node::Upgrade` stopped being a stub. Real H1 upgrade behavior is covered
// end-to-end in `tests/hyper_upgrade.rs`.)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 10. execute_collect_body_before_errors_as_unsupported
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_collect_body_before_errors_as_unsupported() {
	// S1-15 stub: `collect_body_before: Some(_)` returns Error::internal
	// with a "collect_body_before" marker. The surrounding node can be
	// any variant; we use a Middleware node (which need not be registered
	// because the stub fires before dispatch).
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: Some(vane_core::BodySide::Request),
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		// The middleware slab must still parse through link; use a noop that
		// is never driven (the collect_body_before stub fires first).
		vec![l7_req_ref("noop_never_run")],
		vec![],
		vec![Terminator::Close],
	);
	let counter = Arc::new(AtomicUsize::new(0));
	let mut mw = MiddlewareFactories::new();
	{
		let counter = Arc::clone(&counter);
		mw.register("noop_never_run", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&counter)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	let err = result.expect_err("collect_body_before stub must return Err");
	let rendered = err.to_string();
	assert!(
		rendered.contains("collect_body_before"),
		"error must mention collect_body_before; got {rendered:?}",
	);
	// The middleware must never run — the collect stub fires before dispatch.
	assert_eq!(
		counter.load(Ordering::SeqCst),
		0,
		"middleware must not run when collect_body_before errors first",
	);
	// Silence the otherwise-unused sink handle; assertion is on the error.
	let _ = sink.count();
}

// ---------------------------------------------------------------------------
// Verbosity-mode tests (12-15). The existing `run_execute` always builds
// `FlowCtx` with `FlowLogVerbosity::Trajectory`; the helper below is its
// twin that takes the verbosity as an argument so we can drive the
// debug-mode path without disturbing the original ten tests.
// ---------------------------------------------------------------------------

async fn run_execute_with_verbosity(
	graph: &Arc<FlowGraph>,
	entry: NodeId,
	input: ExecutorInput,
	conn: &Arc<ConnContext>,
	sink: &Arc<NullSink>,
	verbosity: FlowLogVerbosity,
) -> Result<vane_engine::executor::ExecutorOutput, Error> {
	let mut ctx = FlowCtx {
		span: tracing::Span::none(),
		log: Arc::clone(sink) as Arc<dyn FlowLogSink>,
		cancel: CancellationToken::new(),
		verbosity,
		trajectory: vane_core::TrajectoryBuilder::new(conn.id, entry, 0),
	};
	execute(graph, entry, input, conn, &mut ctx).await
}

/// Pull the single `FlowLogKind::Trajectory` event out of `sink` and
/// deserialize its `data` field into a `FlowTrajectory`. Panics on any
/// shape violation — the spec (`02-flow.md` § _Flow log verbosity_) says
/// exactly one such event lands per request.
fn extract_trajectory(sink: &NullSink) -> FlowTrajectory {
	let events = sink.events.lock();
	let mut iter = events.iter().filter(|e| e.kind == FlowLogKind::Trajectory);
	let event = iter.next().expect("exactly one Trajectory event must land");
	assert!(iter.next().is_none(), "no more than one Trajectory event per request");
	let data = event.data.as_ref().expect("Trajectory event must carry serialized data");
	serde_json::from_value::<FlowTrajectory>(data.clone()).expect("decode FlowTrajectory")
}

// Two-middleware Continue→Continue→Terminate(Close) graph used by tests 12
// and 13. Inlined per call so the linked counters stay test-local.
fn two_middleware_close_graph(
	a_counter: Arc<AtomicUsize>,
	b_counter: Arc<AtomicUsize>,
) -> Arc<FlowGraph> {
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
			},
			Node::Middleware {
				id: MiddlewareId::new(1),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![l7_req_ref("first_continue"), l7_req_ref("second_continue")],
		vec![],
		vec![Terminator::Close],
	);
	let mut mw = MiddlewareFactories::new();
	{
		let counter = a_counter;
		mw.register("first_continue", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&counter)))))
		});
	}
	{
		let counter = b_counter;
		mw.register("second_continue", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&counter)))))
		});
	}
	let fetch = FetchFactories::new();
	FlowGraph::link(sym, &mw, &fetch).expect("link")
}

// ---------------------------------------------------------------------------
// 12. execute_emits_one_trajectory_event_in_default_mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_emits_one_trajectory_event_in_default_mode() {
	// Spec (02-flow.md § _Flow log verbosity_, Trajectory mode): per
	// request, exactly one `FlowLogKind::Trajectory` event lands in
	// `ctx.log`. Per-step middleware events are suppressed; connection-
	// level milestone events (`Terminate`) still fire.
	let a = Arc::new(AtomicUsize::new(0));
	let b = Arc::new(AtomicUsize::new(0));
	let graph = two_middleware_close_graph(Arc::clone(&a), Arc::clone(&b));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute_with_verbosity(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
		FlowLogVerbosity::Trajectory,
	)
	.await;
	assert!(result.is_ok(), "two-middleware happy path must succeed: {result:?}");
	assert_eq!(a.load(Ordering::SeqCst), 1);
	assert_eq!(b.load(Ordering::SeqCst), 1);

	let kinds = sink.kinds();
	let traj_count = kinds.iter().filter(|k| **k == FlowLogKind::Trajectory).count();
	assert_eq!(traj_count, 1, "exactly one Trajectory event in Trajectory mode; got {kinds:?}");
	let mw_count = kinds.iter().filter(|k| **k == FlowLogKind::Middleware).count();
	assert_eq!(
		mw_count, 0,
		"per-step Middleware events suppressed in Trajectory mode; got {kinds:?}"
	);

	// The trajectory's step list contains the two middleware visits.
	// Terminate is reflected by the outcome, not by an extra step.
	let traj = extract_trajectory(&sink);
	assert_eq!(traj.steps.len(), 2, "two middleware visits → two trajectory steps");
}

// ---------------------------------------------------------------------------
// 13. execute_emits_per_step_events_in_debug_mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_emits_per_step_events_in_debug_mode() {
	// Spec (02-flow.md § _Flow log verbosity_, Debug mode): the per-step
	// stream lands in addition to the trajectory. For the same two-
	// middleware graph this is: 2 Middleware + 1 Terminate (milestone) +
	// 1 Trajectory = 4 total events.
	let a = Arc::new(AtomicUsize::new(0));
	let b = Arc::new(AtomicUsize::new(0));
	let graph = two_middleware_close_graph(Arc::clone(&a), Arc::clone(&b));
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute_with_verbosity(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
		FlowLogVerbosity::Debug,
	)
	.await;
	assert!(result.is_ok(), "Debug mode must not change happy-path outcome: {result:?}");

	let kinds = sink.kinds();
	let traj_count = kinds.iter().filter(|k| **k == FlowLogKind::Trajectory).count();
	let mw_count = kinds.iter().filter(|k| **k == FlowLogKind::Middleware).count();
	let term_count = kinds.iter().filter(|k| **k == FlowLogKind::Terminate).count();
	assert_eq!(traj_count, 1, "still exactly one Trajectory event; got {kinds:?}");
	assert_eq!(mw_count, 2, "two middleware visits → two Middleware events; got {kinds:?}");
	assert_eq!(term_count, 1, "Close terminator emits one Terminate milestone; got {kinds:?}");
	assert_eq!(kinds.len(), 4, "Debug-mode total = 1T + 2M + 1Term; got {kinds:?}");
}

// ---------------------------------------------------------------------------
// 14. execute_trajectory_outcome_records_terminator_kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_trajectory_outcome_records_terminator_kind() {
	// Spec (02-flow.md § _Flow log verbosity_): on the happy path the
	// trajectory's `outcome` is `Terminated { terminator: <kind> }`.
	// `Terminator::Close` maps to `TerminatorOutcomeKind::Close`.
	let sym = build_graph(
		vec![Node::Terminate(TerminatorId::new(0))],
		vec![],
		vec![],
		vec![],
		vec![Terminator::Close],
	);
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute_with_verbosity(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
		FlowLogVerbosity::Trajectory,
	)
	.await;
	assert!(result.is_ok(), "Close terminator must succeed: {result:?}");

	let traj = extract_trajectory(&sink);
	match traj.outcome {
		TrajectoryOutcome::Terminated { terminator, .. } => {
			assert_eq!(
				terminator,
				TerminatorOutcomeKind::Close,
				"Close terminator → TerminatorOutcomeKind::Close",
			);
		}
		other @ TrajectoryOutcome::Error { .. } => {
			panic!("expected Terminated outcome, got {other:?}")
		}
	}
}

// ---------------------------------------------------------------------------
// 15. execute_trajectory_outcome_records_error_when_propagating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_trajectory_outcome_records_error_when_propagating() {
	// Spec (02-flow.md § _Flow log verbosity_): on the error path the
	// trajectory's `outcome` is `Error { message }` whose payload contains
	// the error's Display. With `on_error: None` the Err propagates and
	// the executor must still emit one Trajectory event before returning.
	let counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![l7_req_ref("fail")],
		vec![],
		vec![Terminator::Close],
	);
	let mut mw = MiddlewareFactories::new();
	{
		let counter = Arc::clone(&counter);
		mw.register("fail", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(FailMiddleware(Arc::clone(&counter)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute_with_verbosity(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
		FlowLogVerbosity::Trajectory,
	)
	.await;
	let _ = result.expect_err("Err without on_error must propagate");

	let kinds = sink.kinds();
	let traj_count = kinds.iter().filter(|k| **k == FlowLogKind::Trajectory).count();
	assert_eq!(traj_count, 1, "exactly one Trajectory event even on error path; got {kinds:?}");

	let traj = extract_trajectory(&sink);
	match &traj.outcome {
		TrajectoryOutcome::Error { message, .. } => {
			assert!(
				message.as_ref().contains("simulated"),
				"trajectory error message must contain the middleware's Display payload; got {message:?}",
			);
		}
		other @ TrajectoryOutcome::Terminated { .. } => {
			panic!("expected Error outcome, got {other:?}")
		}
	}
}

// ---------------------------------------------------------------------------
// C8a contract tests (15-20). These pin the ExecutorOutput shape introduced
// in commit 85cfd470: WriteHttpResponse hands back the Response verbatim,
// ByteTunnel drives `tokio::io::copy_bidirectional` to completion and reports
// the close reason out-of-band. Per 05-terminator.md § _Variants_ and
// 02-flow.md § _Execution model_.
// ---------------------------------------------------------------------------

/// `L7Fetch` fixture that returns a caller-supplied `Response`. The response
/// is moved out on first invocation; subsequent calls panic. Used to assert
/// that the executor preserves the exact `Response` value through the
/// `WriteHttpResponse` terminator.
struct CannedResponseFetch(Mutex<Option<Response>>);

#[async_trait]
impl L7Fetch for CannedResponseFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let resp = self.0.lock().take().expect("CannedResponseFetch must be invoked at most once");
		Ok(L7FetchOutput::Response(resp))
	}
}

/// `L4Fetch` fixture that hands the executor a caller-supplied `Tunnel`.
/// Same single-shot semantics as `CannedResponseFetch`.
struct CannedTunnelFetch(Mutex<Option<Tunnel>>);

#[async_trait]
impl L4Fetch for CannedTunnelFetch {
	async fn fetch(
		&self,
		_l4: L4Conn,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Tunnel, Error> {
		let tunnel = self.0.lock().take().expect("CannedTunnelFetch must be invoked at most once");
		Ok(tunnel)
	}
}

/// Build the L4-entry tunnel graph used by tests 17-19:
///   0: `Fetch(L4Forward)` { `next_tunnel` = 1 } -> 1: `Terminate(ByteTunnel)`
fn byte_tunnel_graph(tunnel: Tunnel) -> Arc<FlowGraph> {
	let sym = build_graph(
		vec![
			Node::Fetch {
				id: FetchId::new(0),
				next_response: None,
				next_tunnel: Some(NodeId::new(1)),
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef { kind: FetchKind::L4Forward, args: Value::Null }],
		vec![Terminator::ByteTunnel],
	);
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	let slot = Arc::new(Mutex::new(Some(tunnel)));
	{
		let slot = Arc::clone(&slot);
		fetch.register(FetchKind::L4Forward, move |_args| {
			let tunnel = slot.lock().take().expect("L4Forward factory invoked more than once");
			Ok(FetchInst::L4(Arc::new(CannedTunnelFetch(Mutex::new(Some(tunnel))))))
		});
	}
	FlowGraph::link(sym, &mw, &fetch).expect("link")
}

/// Spin up a throwaway `TcpStream` so the L4 executor branch has a real
/// `L4Conn::Tcp` value to consume. The stream is fed to the L4 fetch
/// factory and dropped there — the bytes never matter; only the type
/// shape does.
async fn throwaway_tcp_stream() -> tokio::net::TcpStream {
	let listener =
		tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral listener");
	let addr = listener.local_addr().expect("local_addr");
	let connect = tokio::net::TcpStream::connect(addr);
	let accept = listener.accept();
	let (client, _server) = tokio::join!(connect, accept);
	client.expect("connect to ephemeral listener")
}

// ---------------------------------------------------------------------------
// 15. execute_write_http_response_returns_response_output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_write_http_response_returns_response_output() {
	// 05-terminator.md § _Variants_: WriteHttpResponse consumes the Response
	// produced by the preceding L7Fetch and hands it to the caller verbatim.
	// The executor must surface `Ok(ExecutorOutput::HttpResponse(r))` whose
	// `r.status()` matches what the fetch produced.
	let fetch_counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(1)),
				next_tunnel: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: Value::Null }],
		vec![Terminator::WriteHttpResponse],
	);
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	{
		let counter = Arc::clone(&fetch_counter);
		fetch.register(FetchKind::HttpSynthesize, move |_args| {
			Ok(FetchInst::L7(Arc::new(SynthOkFetch(Arc::clone(&counter)))))
		});
	}
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	match result {
		Ok(ExecutorOutput::HttpResponse(r)) => {
			assert_eq!(r.status().as_u16(), 200, "WriteHttpResponse must preserve status verbatim");
		}
		other => panic!("expected Ok(ExecutorOutput::HttpResponse), got {other:?}"),
	}
	assert_eq!(fetch_counter.load(Ordering::SeqCst), 1, "fetch must run exactly once");
}

// ---------------------------------------------------------------------------
// 16. execute_write_http_response_preserves_body_payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_write_http_response_preserves_body_payload() {
	// 05-terminator.md § _Variants_: the executor does not mutate the
	// Response. A `Body::Static(Bytes)` body produced by the fetch must
	// arrive at the caller byte-for-byte.
	let canned: Response = http::Response::builder()
		.status(201)
		.body(Body::Static(Bytes::from_static(b"hello")))
		.expect("build canned response");
	let sym = build_graph(
		vec![
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(1)),
				next_tunnel: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: Value::Null }],
		vec![Terminator::WriteHttpResponse],
	);
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	let slot = Arc::new(Mutex::new(Some(canned)));
	{
		let slot = Arc::clone(&slot);
		fetch.register(FetchKind::HttpSynthesize, move |_args| {
			let resp = slot.lock().take().expect("canned response factory invoked twice");
			Ok(FetchInst::L7(Arc::new(CannedResponseFetch(Mutex::new(Some(resp))))))
		});
	}
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	match result {
		Ok(ExecutorOutput::HttpResponse(r)) => {
			assert_eq!(r.status().as_u16(), 201, "status preserved");
			let bytes = r.body().as_static().expect("body must remain Body::Static");
			assert_eq!(bytes.as_ref(), b"hello", "body payload preserved verbatim");
		}
		other => panic!("expected Ok(ExecutorOutput::HttpResponse), got {other:?}"),
	}
}

// ---------------------------------------------------------------------------
// 17. execute_byte_tunnel_drives_copy_bidirectional
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_byte_tunnel_drives_copy_bidirectional() {
	// 05-terminator.md § _Variants_ + 02-flow.md § _Execution model_:
	// `Terminator::ByteTunnel` hands the Tunnel's two halves to
	// `tokio::io::copy_bidirectional`. Bytes written into either outer
	// half must surface on the opposite outer half. The executor returns
	// `Ok(ExecutorOutput::Tunneled)` once both directions reach EOF.
	let (mut client_outer, client_inner) = tokio::io::duplex(1024);
	let (mut upstream_outer, upstream_inner) = tokio::io::duplex(1024);
	let tunnel = Tunnel {
		client: Box::new(client_inner) as Box<dyn AsyncReadWrite + Send>,
		upstream: Box::new(upstream_inner) as Box<dyn AsyncReadWrite + Send>,
		close_reason_tx: None,
	};
	let graph = byte_tunnel_graph(tunnel);
	let conn = make_conn("127.0.0.1:0");
	let l4 = L4Conn::Tcp(throwaway_tcp_stream().await);

	// Spawn the executor; it owns its own NullSink + FlowCtx so the test
	// thread can drive the duplex pairs concurrently.
	let conn_for_exec = Arc::clone(&conn);
	let graph_for_exec = Arc::clone(&graph);
	let executor = tokio::spawn(async move {
		let sink: Arc<dyn FlowLogSink> = Arc::new(NullSink::new());
		let mut ctx = FlowCtx {
			span: tracing::Span::none(),
			log: sink,
			cancel: CancellationToken::new(),
			verbosity: FlowLogVerbosity::Trajectory,
			trajectory: vane_core::TrajectoryBuilder::new(conn_for_exec.id, NodeId::new(0), 0),
		};
		execute(
			&graph_for_exec,
			NodeId::new(0),
			ExecutorInput::L4(Box::new(l4)),
			&conn_for_exec,
			&mut ctx,
		)
		.await
	});

	// Client → upstream direction.
	client_outer.write_all(b"ping").await.expect("write client side");
	client_outer.shutdown().await.expect("shutdown client side");
	let mut from_upstream = Vec::new();
	upstream_outer.read_to_end(&mut from_upstream).await.expect("read upstream side");
	assert_eq!(from_upstream, b"ping", "client→upstream bytes copied verbatim");

	// Upstream → client direction.
	upstream_outer.write_all(b"pong").await.expect("write upstream side");
	upstream_outer.shutdown().await.expect("shutdown upstream side");
	let mut from_client = Vec::new();
	client_outer.read_to_end(&mut from_client).await.expect("read client side");
	assert_eq!(from_client, b"pong", "upstream→client bytes copied verbatim");

	let result = executor.await.expect("executor task panicked");
	match result {
		Ok(ExecutorOutput::Tunneled) => {}
		other => panic!("expected Ok(ExecutorOutput::Tunneled), got {other:?}"),
	}
}

// ---------------------------------------------------------------------------
// 18. execute_byte_tunnel_sends_graceful_close_reason
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_byte_tunnel_sends_graceful_close_reason() {
	// 02-flow.md § _Execution model_ + 05-terminator.md § _Variants_:
	// when both sides EOF cleanly, the executor sends
	// `CloseReason::Graceful` through `Tunnel.close_reason_tx`.
	let (mut client_outer, client_inner) = tokio::io::duplex(1024);
	let (mut upstream_outer, upstream_inner) = tokio::io::duplex(1024);
	let (tx, rx) = tokio::sync::oneshot::channel::<CloseReason>();
	let tunnel = Tunnel {
		client: Box::new(client_inner) as Box<dyn AsyncReadWrite + Send>,
		upstream: Box::new(upstream_inner) as Box<dyn AsyncReadWrite + Send>,
		close_reason_tx: Some(tx),
	};
	let graph = byte_tunnel_graph(tunnel);
	let conn = make_conn("127.0.0.1:0");
	let l4 = L4Conn::Tcp(throwaway_tcp_stream().await);

	let conn_for_exec = Arc::clone(&conn);
	let graph_for_exec = Arc::clone(&graph);
	let executor = tokio::spawn(async move {
		let sink: Arc<dyn FlowLogSink> = Arc::new(NullSink::new());
		let mut ctx = FlowCtx {
			span: tracing::Span::none(),
			log: sink,
			cancel: CancellationToken::new(),
			verbosity: FlowLogVerbosity::Trajectory,
			trajectory: vane_core::TrajectoryBuilder::new(conn_for_exec.id, NodeId::new(0), 0),
		};
		execute(
			&graph_for_exec,
			NodeId::new(0),
			ExecutorInput::L4(Box::new(l4)),
			&conn_for_exec,
			&mut ctx,
		)
		.await
	});

	// Both sides shut down cleanly; copy_bidirectional sees Ok on each half.
	client_outer.shutdown().await.expect("shutdown client");
	upstream_outer.shutdown().await.expect("shutdown upstream");
	// Drain any residual bytes so the inner halves observe EOF without error.
	let mut buf = Vec::new();
	client_outer.read_to_end(&mut buf).await.expect("drain client side");
	upstream_outer.read_to_end(&mut buf).await.expect("drain upstream side");

	let result = executor.await.expect("executor task panicked");
	match result {
		Ok(ExecutorOutput::Tunneled) => {}
		other => panic!("expected Ok(ExecutorOutput::Tunneled), got {other:?}"),
	}
	let reason = rx.await.expect("close_reason_tx must fire on graceful EOF");
	match reason {
		CloseReason::Graceful => {}
		other => panic!("expected CloseReason::Graceful, got {other:?}"),
	}
}

// ---------------------------------------------------------------------------
// 19. execute_byte_tunnel_propagates_io_error_via_close_reason
// ---------------------------------------------------------------------------

/// `AsyncRead` impl that errors on every read. Paired with a no-op `AsyncWrite`
/// so it satisfies `AsyncReadWrite + Send + Unpin`. Used to force
/// `tokio::io::copy_bidirectional` into the io-error branch.
struct ErrorOnRead;

impl AsyncRead for ErrorOnRead {
	fn poll_read(
		self: Pin<&mut Self>,
		_cx: &mut Context<'_>,
		_buf: &mut ReadBuf<'_>,
	) -> Poll<io::Result<()>> {
		Poll::Ready(Err(io::Error::other("synthetic")))
	}
}

impl AsyncWrite for ErrorOnRead {
	fn poll_write(
		self: Pin<&mut Self>,
		_cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<io::Result<usize>> {
		Poll::Ready(Ok(buf.len()))
	}

	fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}

	fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
}

#[tokio::test]
async fn execute_byte_tunnel_propagates_io_error_via_close_reason() {
	// 02-flow.md § _Execution model_ + 05-terminator.md § _Variants_ + this
	// chunk's behavior contract: when the inner copy_bidirectional returns
	// Err, the executor sends `CloseReason::ProtocolError(_)` through
	// `Tunnel.close_reason_tx` and STILL returns
	// `Ok(ExecutorOutput::Tunneled)` — io errors do not bubble out as
	// walker `Err`.
	let (tx, rx) = tokio::sync::oneshot::channel::<CloseReason>();
	let (_upstream_outer, upstream_inner) = tokio::io::duplex(1024);
	let tunnel = Tunnel {
		client: Box::new(ErrorOnRead) as Box<dyn AsyncReadWrite + Send>,
		upstream: Box::new(upstream_inner) as Box<dyn AsyncReadWrite + Send>,
		close_reason_tx: Some(tx),
	};
	let graph = byte_tunnel_graph(tunnel);
	let conn = make_conn("127.0.0.1:0");
	let l4 = L4Conn::Tcp(throwaway_tcp_stream().await);
	let sink = Arc::new(NullSink::new());

	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L4(Box::new(l4)), &conn, &sink).await;

	match result {
		Ok(ExecutorOutput::Tunneled) => {}
		other => panic!("io errors must NOT bubble out; got {other:?}"),
	}
	let reason = rx.await.expect("close_reason_tx must fire on io error");
	match reason {
		CloseReason::ProtocolError(_) => {}
		other => panic!("expected CloseReason::ProtocolError, got {other:?}"),
	}
}

// ---------------------------------------------------------------------------
// 20. execute_close_terminator_returns_closed_output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_close_terminator_returns_closed_output() {
	// Entry = Terminate(Close). 05-terminator.md § _Variants_: Close drops
	// the transport silently and emits a FlowLogKind::Terminate event.
	// Per the C8a contract the precise success value is
	// `ExecutorOutput::Closed`.
	let sym = build_graph(
		vec![Node::Terminate(TerminatorId::new(0))],
		vec![],
		vec![],
		vec![],
		vec![Terminator::Close],
	);
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result = run_execute(
		&graph,
		NodeId::new(0),
		ExecutorInput::L7(Box::new(empty_l7_request())),
		&conn,
		&sink,
	)
	.await;

	assert!(
		matches!(result, Ok(ExecutorOutput::Closed)),
		"Close terminator must return Ok(ExecutorOutput::Closed); got {result:?}",
	);
	let kinds = sink.kinds();
	assert!(
		kinds.contains(&FlowLogKind::Terminate),
		"expected at least one Terminate event, got {kinds:?}",
	);
}

// ---------------------------------------------------------------------------
// 21. execute_byte_tunnel_terminates_with_cancelled_close_reason_on_ctx_cancel
// ---------------------------------------------------------------------------

/// L4 fetch fixture that pulses `notify_one` immediately before handing
/// the canned `Tunnel` to the executor. The test's main task awaits
/// `notified()` to anchor on "fetch resolved → executor about to enter
/// `Terminator::ByteTunnel`'s `copy_bidirectional`," eliminating the
/// timing-based `tokio::time::sleep` the previous revision relied on.
struct NotifyingTunnelFetch {
	tunnel: Mutex<Option<Tunnel>>,
	notify: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl L4Fetch for NotifyingTunnelFetch {
	async fn fetch(
		&self,
		_l4: L4Conn,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Tunnel, Error> {
		let tunnel = self.tunnel.lock().take().expect("NotifyingTunnelFetch invoked more than once");
		self.notify.notify_one();
		Ok(tunnel)
	}
}

fn byte_tunnel_graph_with_notify(
	tunnel: Tunnel,
	notify: &Arc<tokio::sync::Notify>,
) -> Arc<FlowGraph> {
	let sym = build_graph(
		vec![
			Node::Fetch {
				id: FetchId::new(0),
				next_response: None,
				next_tunnel: Some(NodeId::new(1)),
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef { kind: FetchKind::L4Forward, args: Value::Null }],
		vec![Terminator::ByteTunnel],
	);
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	let slot = Arc::new(Mutex::new(Some(tunnel)));
	{
		let slot = Arc::clone(&slot);
		let notify = Arc::clone(notify);
		fetch.register(FetchKind::L4Forward, move |_args| {
			let tunnel = slot.lock().take().expect("L4Forward factory invoked more than once");
			Ok(FetchInst::L4(Arc::new(NotifyingTunnelFetch {
				tunnel: Mutex::new(Some(tunnel)),
				notify: Arc::clone(&notify),
			})))
		});
	}
	FlowGraph::link(sym, &mw, &fetch).expect("link")
}

#[tokio::test]
async fn execute_byte_tunnel_terminates_with_cancelled_close_reason_on_ctx_cancel() {
	// 01-topology.md § _Listener lifecycle_ step 3 + 05-terminator.md §
	// _Variants_: when `ctx.cancel.cancelled()` fires while a `ByteTunnel`
	// is mid-copy, the executor's biased `tokio::select!` exits the copy,
	// sends `CloseReason::Cancelled` through `Tunnel.close_reason_tx`, and
	// returns `Ok(ExecutorOutput::Tunneled)`. The duplex halves are kept
	// open so `tokio::io::copy_bidirectional` would otherwise block forever
	// — only cancellation can unblock the executor.
	//
	// Synchronisation: the L4 fetch fixture pulses `notify_one` before
	// returning the `Tunnel`. The test thread awaits `notified()` so it
	// only fires `cancel` once the executor has actually consumed the
	// fetch result. A single `yield_now()` then lets the executor advance
	// from "fetch returned" to "parked in `copy_bidirectional`'s
	// `tokio::select!`" before the cancel hits — replacing the wall-clock
	// sleep the prior revision relied on.
	let (_client_outer, client_inner) = tokio::io::duplex(1024);
	let (_upstream_outer, upstream_inner) = tokio::io::duplex(1024);
	let (close_tx, close_rx) = tokio::sync::oneshot::channel::<CloseReason>();
	let tunnel = Tunnel {
		client: Box::new(client_inner) as Box<dyn AsyncReadWrite + Send>,
		upstream: Box::new(upstream_inner) as Box<dyn AsyncReadWrite + Send>,
		close_reason_tx: Some(close_tx),
	};
	let notify = Arc::new(tokio::sync::Notify::new());
	let graph = byte_tunnel_graph_with_notify(tunnel, &notify);
	let conn = make_conn("127.0.0.1:0");
	let l4 = L4Conn::Tcp(throwaway_tcp_stream().await);

	let cancel = CancellationToken::new();
	let cancel_for_exec = cancel.clone();
	let conn_for_exec = Arc::clone(&conn);
	let graph_for_exec = Arc::clone(&graph);
	let executor = tokio::spawn(async move {
		let sink: Arc<dyn FlowLogSink> = Arc::new(NullSink::new());
		let mut ctx = FlowCtx {
			span: tracing::Span::none(),
			log: sink,
			cancel: cancel_for_exec,
			verbosity: FlowLogVerbosity::Trajectory,
			trajectory: vane_core::TrajectoryBuilder::new(conn_for_exec.id, NodeId::new(0), 0),
		};
		execute(
			&graph_for_exec,
			NodeId::new(0),
			ExecutorInput::L4(Box::new(l4)),
			&conn_for_exec,
			&mut ctx,
		)
		.await
	});

	// Anchor on "fetch resolved" instead of wall-clock time. After notify,
	// yield once so the executor can step from the fetch return into the
	// `copy_bidirectional` select! before the cancel arrives.
	notify.notified().await;
	tokio::task::yield_now().await;
	cancel.cancel();

	let result = executor.await.expect("executor task panicked");
	match result {
		Ok(ExecutorOutput::Tunneled) => {}
		other => panic!("cancelled tunnel must return Ok(ExecutorOutput::Tunneled); got {other:?}"),
	}
	let reason = close_rx.await.expect("close_reason_tx must fire on cancel");
	match reason {
		CloseReason::Cancelled => {}
		other => panic!("expected CloseReason::Cancelled, got {other:?}"),
	}
}
