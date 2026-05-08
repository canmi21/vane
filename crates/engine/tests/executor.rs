//! Integration tests for `vane_engine::executor::execute`.
//!
//! Covers the execution-model contract described in
//! `spec/flow-model.md` § _Executor_ (lines 330-469), the
//! middleware two-channel routing described in
//! `spec/crates/engine.md` § _Middleware_ / _Two error channels,
//! not one_, and the three Terminator variants in
//! `spec/crates/engine.md`.
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
	L7FetchOutput, L7RequestMiddleware, L7ResponseMiddleware, MiddlewareId, MiddlewareKind, Node,
	NodeId, PredicateId, PredicateInst, Request, Response, ShortCircuit, SymbolicFetchRef,
	SymbolicFlowGraph, SymbolicMiddlewareRef, Terminator, TerminatorId, TerminatorOutcomeKind,
	TrajectoryOutcome, Transport, Tunnel,
};
use vane_engine::executor::{ExecutorInput, ExecutorOutput, execute};
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FetchInst, FlowGraph, MiddlewareInst};

// Fixtures: log sink + ConnContext / FlowCtx builders.

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

	#[allow(dead_code)]
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
		short_circuit_response_entry: std::collections::BTreeMap::new(),
		listener_tls: std::collections::BTreeMap::new(),
		listener_kinds: std::collections::BTreeMap::new(),

		listener_transports: std::collections::BTreeMap::new(),
		annotations: Vec::new(),
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

// Middleware fixtures. Each counts invocations via an `AtomicUsize` so tests
// can assert exact call counts without inspecting executor internals.

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

struct ShortCloseProtocolError(std::borrow::Cow<'static, str>);

#[async_trait]
impl L7RequestMiddleware for ShortCloseProtocolError {
	async fn run(
		&self,
		_req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		Ok(Decision::Short(ShortCircuit::Close(CloseReason::ProtocolError(self.0.clone()))))
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

// Helper: drive `execute` against a linked graph. Tests call this with a
// fresh sink + context per invocation.

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

// 1. execute_middleware_continue_advances_cursor

#[tokio::test]
async fn execute_middleware_continue_advances_cursor() {
	// 3-node graph: Middleware(L7Request) -> Terminate(Close). The middleware
	// returns Decision::Continue; the cursor must advance to `next`.
	// Per spec/flow-model.md § _Executor_ (lines 391-392):
	//   Ok(Decision::Continue) → cur = *next
	let counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
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

// 3. execute_middleware_short_close_policy_denied_returns_closed

#[tokio::test]
async fn execute_middleware_short_close_policy_denied_returns_closed() {
	// spec/flow-model.md § _Executor_:
	// `Short(Close(PolicyDenied(_)))` is a routing-level refusal, not an
	// error. The executor returns `Ok(ExecutorOutput::Closed)`; downstream
	// the H1 service-fn maps that to 404 + `Connection: close`, and the L4
	// listener drops the socket. Trajectory carries
	// `Terminated { terminator: Close }` — uniform with the synth-default
	// `Terminate(Close)` arm so wire-level and log-level behaviour match.
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
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

	assert!(
		matches!(result, Ok(ExecutorOutput::Closed)),
		"Short(Close(PolicyDenied)) must surface as Ok(Closed); got {result:?}",
	);
	let kinds = sink.kinds();
	assert!(
		kinds.contains(&FlowLogKind::Terminate),
		"PolicyDenied path must emit a Terminate milestone; got {kinds:?}",
	);
	assert!(
		kinds.contains(&FlowLogKind::Trajectory),
		"trajectory must finalise on a Short(Close) exit; got {kinds:?}",
	);
}

#[tokio::test]
async fn execute_middleware_short_close_protocol_error_returns_err() {
	// Sibling of the PolicyDenied test: `ProtocolError` is the only
	// `CloseReason` variant that maps back to `Err`. It reaches the H1
	// service-fn as Err and surfaces as 500.
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![l7_req_ref("short_close_protocol_err")],
		vec![],
		vec![Terminator::Close],
	);
	let mut mw = MiddlewareFactories::new();
	mw.register("short_close_protocol_err", MiddlewareKind::L7Request, |_args| {
		Ok(MiddlewareInst::L7Request(Arc::new(ShortCloseProtocolError(std::borrow::Cow::Borrowed(
			"client framing busted",
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

	let err = result.expect_err("Short(Close(ProtocolError)) must surface as Err");
	let rendered = err.to_string();
	assert!(
		rendered.contains("client framing busted"),
		"Err Display must carry the reason payload; got {rendered:?}",
	);
}

// 4. execute_middleware_err_routes_via_on_error

#[tokio::test]
async fn execute_middleware_err_routes_via_on_error() {
	// `spec/flow-model.md` § _Two error channels_: `Err(_)` with
	// `on_error = Some(t)` jumps to `t`; the `Err` does not propagate.
	// Here the target is a Close terminator, so execute returns Ok.
	let counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				// on_error points at the terminator at NodeId(1).
				on_error: Some(NodeId::new(1)),
				collect_body_before: None,
				body_limit: 0,
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

// 5. execute_middleware_err_without_on_error_propagates

#[tokio::test]
async fn execute_middleware_err_without_on_error_propagates() {
	// Without `on_error`, the executor contract (per
	// `spec/crates/engine.md` § _Middleware_) lets `Err` propagate
	// verbatim. Assert the simulated message shows in to_string().
	let counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
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

// 6. execute_check_routes_by_predicate_remote_ip_equals

#[tokio::test]
async fn execute_check_routes_by_predicate_remote_ip_equals() {
	// spec/flow-model.md § _Executor_: Check builds a PredicateView, tests
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
				body_limit: 0,
			},
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
			Node::Middleware {
				id: MiddlewareId::new(1),
				next: NodeId::new(4),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
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

// 7. execute_check_routes_by_predicate_http_method_equals

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
				body_limit: 0,
			},
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
			Node::Middleware {
				id: MiddlewareId::new(1),
				next: NodeId::new(4),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
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

// 8. execute_l7_fetch_response_jumps_to_next_response

#[tokio::test]
async fn execute_l7_fetch_response_jumps_to_next_response() {
	// spec/flow-model.md § _Executor_ (lines 411-424): an L7 Fetch that
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
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
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

// (Test 9 — `execute_upgrade_node_errors_as_unsupported` — was removed when
// `Node::Upgrade` stopped being a stub. Real H1 upgrade behavior is covered
// end-to-end in `tests/hyper_upgrade.rs`.)

// 10. execute_collect_body_static_is_noop

#[tokio::test]
async fn execute_collect_body_static_is_noop() {
	// collect_body_before: Some(Request) on a Body::Static should be a no-op —
	// the body is already collected, the middleware runs normally.
	let counter = Arc::new(AtomicUsize::new(0));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: Some(vane_core::BodySide::Request),
				body_limit: 8 * 1024 * 1024,
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

	let req: Request = http::Request::builder()
		.method("GET")
		.uri("/")
		.body(Body::Static(Bytes::from_static(b"hello")))
		.expect("build req");
	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(req)), &conn, &sink).await;

	assert!(result.is_ok(), "Static body collect must be a no-op, middleware proceeds: {result:?}");
	assert_eq!(counter.load(Ordering::SeqCst), 1, "middleware must run once");
}

// Verbosity-mode tests (12-15). The existing `run_execute` always builds
// `FlowCtx` with `FlowLogVerbosity::Trajectory`; the helper below is its
// twin that takes the verbosity as an argument so we can drive the
// debug-mode path without disturbing the original ten tests.

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
/// shape violation — the spec (`spec/flow-model.md` § _Flow log verbosity_) says
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
				body_limit: 0,
			},
			Node::Middleware {
				id: MiddlewareId::new(1),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
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

// 12. execute_emits_one_trajectory_event_in_default_mode

#[tokio::test]
async fn execute_emits_one_trajectory_event_in_default_mode() {
	// Spec (spec/flow-model.md § _Flow log verbosity_, Trajectory mode): per
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

// 13. execute_emits_per_step_events_in_debug_mode

#[tokio::test]
async fn execute_emits_per_step_events_in_debug_mode() {
	// Spec (spec/flow-model.md § _Flow log verbosity_, Debug mode): the per-step
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

// 14. execute_trajectory_outcome_records_terminator_kind

#[tokio::test]
async fn execute_trajectory_outcome_records_terminator_kind() {
	// Spec (spec/flow-model.md § _Flow log verbosity_): on the happy path the
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

// 15. execute_trajectory_outcome_records_error_when_propagating

#[tokio::test]
async fn execute_trajectory_outcome_records_error_when_propagating() {
	// Spec (spec/flow-model.md § _Flow log verbosity_): on the error path the
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
				body_limit: 0,
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

// C8a contract tests (15-20). These pin the ExecutorOutput shape introduced
// in commit 85cfd470: WriteHttpResponse hands back the Response verbatim,
// ByteTunnel drives `tokio::io::copy_bidirectional` to completion and reports
// the close reason out-of-band. Per spec/crates/engine.md § _Concrete fetches_ and
// spec/flow-model.md § _Executor_.

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
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef {
			kind: FetchKind::L4Forward,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
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

// 15. execute_write_http_response_returns_response_output

#[tokio::test]
async fn execute_write_http_response_returns_response_output() {
	// spec/crates/engine.md § _Concrete fetches_: WriteHttpResponse consumes the Response
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
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
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

// 16. execute_write_http_response_preserves_body_payload

#[tokio::test]
async fn execute_write_http_response_preserves_body_payload() {
	// spec/crates/engine.md § _Concrete fetches_: the executor does not mutate the
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
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
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

// 17. execute_byte_tunnel_drives_copy_bidirectional

#[tokio::test]
async fn execute_byte_tunnel_drives_copy_bidirectional() {
	// spec/crates/engine.md § _Concrete fetches_ + spec/flow-model.md § _Executor_:
	// `Terminator::ByteTunnel` hands the Tunnel's two halves to
	// `tokio::io::copy_bidirectional`. Bytes written into either outer
	// half must surface on the opposite outer half. The executor returns
	// `Ok(ExecutorOutput::Tunneled)` once both directions reach EOF.
	let (mut client_outer, client_inner) = tokio::io::duplex(1024);
	let (mut upstream_outer, upstream_inner) = tokio::io::duplex(1024);
	let tunnel = Tunnel::Bidi {
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

// 18. execute_byte_tunnel_sends_graceful_close_reason

#[tokio::test]
async fn execute_byte_tunnel_sends_graceful_close_reason() {
	// `spec/flow-model.md` § _Executor_; `spec/crates/engine.md` § _Concrete fetches_:
	// when both sides EOF cleanly, the executor sends
	// `CloseReason::Graceful` through `Tunnel.close_reason_tx`.
	let (mut client_outer, client_inner) = tokio::io::duplex(1024);
	let (mut upstream_outer, upstream_inner) = tokio::io::duplex(1024);
	let (tx, rx) = tokio::sync::oneshot::channel::<CloseReason>();
	let tunnel = Tunnel::Bidi {
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

// 19. execute_byte_tunnel_propagates_io_error_via_close_reason

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
	// Per `spec/crates/engine.md` § _Concrete fetches_ (and `spec/flow-model.md` § _Executor_): this
	// chunk's behavior contract: when the inner copy_bidirectional returns
	// Err, the executor sends `CloseReason::ProtocolError(_)` through
	// `Tunnel.close_reason_tx` and STILL returns
	// `Ok(ExecutorOutput::Tunneled)` — io errors do not bubble out as
	// walker `Err`.
	let (tx, rx) = tokio::sync::oneshot::channel::<CloseReason>();
	let (_upstream_outer, upstream_inner) = tokio::io::duplex(1024);
	let tunnel = Tunnel::Bidi {
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

// 20. execute_close_terminator_returns_closed_output

#[tokio::test]
async fn execute_close_terminator_returns_closed_output() {
	// Entry = Terminate(Close). spec/crates/engine.md § _Concrete fetches_: Close drops
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

// 21. execute_byte_tunnel_terminates_with_cancelled_close_reason_on_ctx_cancel

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
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![],
		vec![SymbolicFetchRef {
			kind: FetchKind::L4Forward,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
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
	// spec/topology.md § _Listener lifecycle_ step 3 + spec/crates/engine.md §
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
	let tunnel = Tunnel::Bidi {
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

// Short(Response) routing through `meta.short_circuit_response_entry`

/// Fixture middleware that returns `Decision::Short(ShortCircuit::Response)`
/// with a fixed status. The executor must set the response slot, jump to
/// the listener-level synth target, and fall through to the
/// `WriteHttpResponse` write path.
struct ShortResponseFixed(u16);

#[async_trait]
impl L7RequestMiddleware for ShortResponseFixed {
	async fn run(
		&self,
		_req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let resp = http::Response::builder().status(self.0).body(Body::Empty).expect("build resp");
		Ok(Decision::Short(ShortCircuit::Response(resp)))
	}
}

#[tokio::test]
async fn execute_middleware_short_response_routes_through_synth_target() {
	// spec/flow-model.md § _Executor_ + § _The compiled form_: an L7
	// request middleware returning Short(Response) sets the response
	// slot and jumps to the synth Terminate(WriteHttpResponse) keyed
	// off `meta.short_circuit_response_entry[entry]`. Result: the
	// executor returns Ok(HttpResponse(r)) carrying the middleware's
	// Response — same wire shape as a normal proxy hit.
	let entry = NodeId::new(0);
	let synth = NodeId::new(2);
	let nodes = vec![
		// Entry: the L7 middleware that emits Short(Response).
		Node::Middleware {
			id: MiddlewareId::new(0),
			next: NodeId::new(1),
			on_error: None,
			collect_body_before: None,
			body_limit: 0,
		},
		// Unreachable on the Short(Response) path; included to keep the
		// chain shape that a real `lower_rule` produces.
		Node::Terminate(TerminatorId::new(0)),
		// Synth Terminate(WriteHttpResponse) — the executor jumps here.
		Node::Terminate(TerminatorId::new(1)),
	];
	let mut sym = build_graph(
		nodes,
		vec![],
		vec![l7_req_ref("short_response_418")],
		vec![],
		vec![Terminator::Close, Terminator::WriteHttpResponse],
	);
	let mut sym_mut = (*sym).clone();
	sym_mut.meta.short_circuit_response_entry.insert(entry, synth);
	sym = Arc::new(sym_mut);

	let mut mw = MiddlewareFactories::new();
	mw.register("short_response_418", MiddlewareKind::L7Request, |_args| {
		Ok(MiddlewareInst::L7Request(Arc::new(ShortResponseFixed(418))))
	});
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let result =
		run_execute(&graph, entry, ExecutorInput::L7(Box::new(empty_l7_request())), &conn, &sink)
			.await
			.expect("Short(Response) routes cleanly");
	match result {
		ExecutorOutput::HttpResponse(r) => {
			assert_eq!(r.status().as_u16(), 418, "synth target wrote middleware's response");
		}
		other => panic!("expected HttpResponse, got {other:?}"),
	}
}

#[tokio::test]
async fn execute_short_circuit_response_with_no_synth_target_errors() {
	// If the meta map is missing an entry that an L7 chain emits a
	// Short(Response) for, the executor surfaces an internal error
	// rather than panicking. This is a regression guard — a future
	// `lower` change that forgets to populate the map should fail
	// loud at runtime, not silently mis-route.
	let entry = NodeId::new(0);
	let nodes = vec![
		Node::Middleware {
			id: MiddlewareId::new(0),
			next: NodeId::new(1),
			on_error: None,
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Terminate(TerminatorId::new(0)),
	];
	// Note: NO `short_circuit_response_entry` insertion.
	let sym = build_graph(
		nodes,
		vec![],
		vec![l7_req_ref("short_response_418")],
		vec![],
		vec![Terminator::Close],
	);

	let mut mw = MiddlewareFactories::new();
	mw.register("short_response_418", MiddlewareKind::L7Request, |_args| {
		Ok(MiddlewareInst::L7Request(Arc::new(ShortResponseFixed(418))))
	});
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let err =
		run_execute(&graph, entry, ExecutorInput::L7(Box::new(empty_l7_request())), &conn, &sink)
			.await
			.expect_err("missing synth target must error");
	assert!(
		err.to_string().contains("lower invariant violated"),
		"error names the lower invariant: {err}",
	);
}

// Helpers for LazyBuffer / body-collect tests

/// Single-frame `HttpBody` that yields one data frame then EOF.
/// Used to build `Body::Stream` without any external stream crate.
struct OnceBody {
	data: Option<Bytes>,
}

impl http_body::Body for OnceBody {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		self: Pin<&mut Self>,
		_cx: &mut Context<'_>,
	) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
		let this = self.get_mut();
		match this.data.take() {
			Some(b) => Poll::Ready(Some(Ok(http_body::Frame::data(b)))),
			None => Poll::Ready(None),
		}
	}
}

/// Build a `Body::Stream` from a static byte slice. Used to test that
/// `collect_body_before` drains the stream into `Body::Static`.
fn stream_body(data: &'static [u8]) -> Body {
	Body::from_producer(OnceBody { data: Some(Bytes::from_static(data)) })
}

/// Middleware that captures whether the body it received was buffered.
/// `true` = `Body::Static` or `Body::Empty`, `false` = `Body::Stream`.
struct BodyCapture(Arc<Mutex<Option<bool>>>);

#[async_trait]
impl L7RequestMiddleware for BodyCapture {
	async fn run(
		&self,
		req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let is_buffered = !matches!(req.body(), Body::Stream(_));
		*self.0.lock() = Some(is_buffered);
		Ok(Decision::Continue)
	}
}

struct ResponseBodyCapture(Arc<Mutex<Option<bool>>>);

#[async_trait]
impl L7ResponseMiddleware for ResponseBodyCapture {
	async fn run(
		&self,
		resp: &mut Response,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let is_buffered = !matches!(resp.body(), Body::Stream(_));
		*self.0.lock() = Some(is_buffered);
		Ok(Decision::Continue)
	}
}

fn l7_resp_ref(name: &str) -> SymbolicMiddlewareRef {
	SymbolicMiddlewareRef {
		name: Arc::from(name),
		args: Value::Null,
		kind: MiddlewareKind::L7Response,
		stateless: true,
		needs_body: false,
		on_error: None,
	}
}

/// `L7Fetch` fixture that returns a stream-body response.
struct StreamResponseFetch;

#[async_trait]
impl L7Fetch for StreamResponseFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let resp: Response = http::Response::builder()
			.status(200)
			.body(stream_body(b"response-data"))
			.expect("build resp");
		Ok(L7FetchOutput::Response(resp))
	}
}

// LazyBuffer collect tests

#[tokio::test]
async fn execute_collect_request_stream_body_becomes_static() {
	// collect_body_before = Some(Request) on a Body::Stream must drain the
	// stream into Body::Static before the middleware runs.
	let captured = Arc::new(Mutex::new(None::<bool>));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: Some(vane_core::BodySide::Request),
				body_limit: 8 * 1024 * 1024,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![SymbolicMiddlewareRef {
			name: Arc::from("body_capture"),
			args: Value::Null,
			kind: MiddlewareKind::L7Request,
			stateless: true,
			needs_body: true,
			on_error: None,
		}],
		vec![],
		vec![Terminator::Close],
	);
	let mut mw = MiddlewareFactories::new();
	{
		let cap = Arc::clone(&captured);
		mw.register("body_capture", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(BodyCapture(Arc::clone(&cap)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let req: Request = http::Request::builder()
		.method("POST")
		.uri("/")
		.body(stream_body(b"streamed-data"))
		.expect("build req");
	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(req)), &conn, &sink).await;
	assert!(result.is_ok(), "stream collection must succeed: {result:?}");
	assert_eq!(*captured.lock(), Some(true), "middleware must receive Body::Static after collection");
}

#[tokio::test]
async fn execute_collect_request_body_over_limit_returns_413() {
	// When the request body exceeds body_limit, the executor short-circuits
	// to the synth WriteHttpResponse terminator with a 413 response.
	let entry = NodeId::new(0);
	let synth = NodeId::new(2);
	let nodes = vec![
		Node::Middleware {
			id: MiddlewareId::new(0),
			next: NodeId::new(1),
			on_error: None,
			collect_body_before: Some(vane_core::BodySide::Request),
			body_limit: 4, // tiny limit — "streamed-data" is 13 bytes
		},
		Node::Terminate(TerminatorId::new(0)),
		Node::Terminate(TerminatorId::new(1)),
	];
	let mut sym = build_graph(
		nodes,
		vec![],
		vec![l7_req_ref("never_runs")],
		vec![],
		vec![Terminator::Close, Terminator::WriteHttpResponse],
	);
	let mut sym_mut = (*sym).clone();
	sym_mut.meta.short_circuit_response_entry.insert(entry, synth);
	sym = Arc::new(sym_mut);

	let counter = Arc::new(AtomicUsize::new(0));
	let mut mw = MiddlewareFactories::new();
	{
		let counter = Arc::clone(&counter);
		mw.register("never_runs", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(CountAndContinue(Arc::clone(&counter)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let req: Request = http::Request::builder()
		.method("POST")
		.uri("/")
		.body(stream_body(b"streamed-data"))
		.expect("build req");
	let result = run_execute(&graph, entry, ExecutorInput::L7(Box::new(req)), &conn, &sink).await;

	match result {
		Ok(ExecutorOutput::HttpResponse(r)) => {
			assert_eq!(r.status().as_u16(), 413, "over-limit request body must yield 413");
		}
		other => panic!("expected 413 HttpResponse, got {other:?}"),
	}
	assert_eq!(counter.load(Ordering::SeqCst), 0, "middleware must not run when body limit exceeded");
}

#[tokio::test]
async fn execute_collect_response_stream_body_becomes_static() {
	// collect_body_before = Some(Response) on a Body::Stream must drain the
	// stream into Body::Static before the L7Response middleware runs.
	//
	// Graph: Fetch(0) -> Middleware(L7Response, collect_body=Response)(1) -> Terminate(2)
	let captured = Arc::new(Mutex::new(None::<bool>));
	let sym = build_graph(
		vec![
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(1)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: Some(vane_core::BodySide::Response),
				body_limit: 8 * 1024 * 1024,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![SymbolicMiddlewareRef {
			name: Arc::from("resp_body_capture"),
			args: Value::Null,
			kind: MiddlewareKind::L7Response,
			stateless: true,
			needs_body: true,
			on_error: None,
		}],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		vec![Terminator::WriteHttpResponse],
	);

	let mut mw = MiddlewareFactories::new();
	{
		let cap = Arc::clone(&captured);
		mw.register("resp_body_capture", MiddlewareKind::L7Response, move |_args| {
			Ok(MiddlewareInst::L7Response(Arc::new(ResponseBodyCapture(Arc::clone(&cap)))))
		});
	}
	let mut fetch = FetchFactories::new();
	fetch
		.register(FetchKind::HttpSynthesize, |_args| Ok(FetchInst::L7(Arc::new(StreamResponseFetch))));
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
	assert!(result.is_ok(), "response stream collection must succeed: {result:?}");
	assert_eq!(
		*captured.lock(),
		Some(true),
		"L7Response middleware must receive Body::Static after collection",
	);
}

#[tokio::test]
async fn execute_collect_response_body_over_limit_returns_err() {
	// When the response body exceeds body_limit, the executor returns Err
	// (upstream protocol violation — 502 semantics via Error::upstream(Malformed)).
	let sym = build_graph(
		vec![
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(1)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: Some(vane_core::BodySide::Response),
				body_limit: 4, // tiny limit
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![l7_resp_ref("never_runs_resp")],
		vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		vec![Terminator::WriteHttpResponse],
	);

	let mut mw = MiddlewareFactories::new();
	mw.register("never_runs_resp", MiddlewareKind::L7Response, |_args| {
		Ok(MiddlewareInst::L7Response(Arc::new(ResponseBodyCapture(Arc::new(Mutex::new(None))))))
	});
	let mut fetch = FetchFactories::new();
	fetch
		.register(FetchKind::HttpSynthesize, |_args| Ok(FetchInst::L7(Arc::new(StreamResponseFetch))));
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

	assert!(result.is_err(), "over-limit response body must return Err: {result:?}");
}

// http.body predicate routing tests

// Graph layout for http.body routing tests:
//   0: Check { predicate:0, on_match:1, on_miss:3, collect_body_before:Request }
//   1: Middleware(hit_match) -> 2
//   2: Terminate(Close)
//   3: Middleware(hit_miss)  -> 4
//   4: Terminate(Close)
fn body_routing_graph(pred: PredicateInst) -> Arc<SymbolicFlowGraph> {
	build_graph(
		vec![
			Node::Check {
				predicate: PredicateId::new(0),
				on_match: NodeId::new(1),
				on_miss: NodeId::new(3),
				collect_body_before: Some(vane_core::BodySide::Request),
				body_limit: 8 * 1024 * 1024,
			},
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(2),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
			Node::Middleware {
				id: MiddlewareId::new(1),
				next: NodeId::new(4),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![pred],
		vec![l7_req_ref("hit_match"), l7_req_ref("hit_miss")],
		vec![],
		vec![Terminator::Close],
	)
}

// Link sym with hit_match/hit_miss counters and drive execute against it.
// Returns (hit_match_count, hit_miss_count).
async fn run_body_routing(sym: Arc<SymbolicFlowGraph>, req: Request) -> (usize, usize) {
	let hit_match = Arc::new(AtomicUsize::new(0));
	let hit_miss = Arc::new(AtomicUsize::new(0));

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
	let sink = Arc::new(NullSink::new());

	let r = run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(req)), &conn, &sink).await;
	assert!(r.is_ok(), "execute must succeed: {r:?}");
	(hit_match.load(Ordering::SeqCst), hit_miss.load(Ordering::SeqCst))
}

#[tokio::test]
async fn execute_http_body_contains_match_routes_on_match() {
	// collect_body_before drains the stream into Body::Static before the
	// Check runs; test_bytes / contains finds "hello" in the body → on_match.
	let sym = body_routing_graph(PredicateInst {
		path: FieldPath::HttpBody,
		op: CompiledOperator::Contains(Bytes::from_static(b"hello")),
	});
	let req = http::Request::builder()
		.method("POST")
		.uri("/")
		.body(stream_body(b"hello world"))
		.expect("build req");
	let (matched, missed) = run_body_routing(sym, req).await;
	assert_eq!(matched, 1, "on_match must fire when body contains target bytes");
	assert_eq!(missed, 0, "on_miss must not fire");
}

#[tokio::test]
async fn execute_http_body_contains_no_match_routes_on_miss() {
	// Body does not contain the target bytes → on_miss branch.
	let sym = body_routing_graph(PredicateInst {
		path: FieldPath::HttpBody,
		op: CompiledOperator::Contains(Bytes::from_static(b"hello")),
	});
	let req = http::Request::builder()
		.method("POST")
		.uri("/")
		.body(stream_body(b"goodbye world"))
		.expect("build req");
	let (matched, missed) = run_body_routing(sym, req).await;
	assert_eq!(matched, 0, "on_match must not fire when body lacks target bytes");
	assert_eq!(missed, 1, "on_miss must fire");
}

#[tokio::test]
async fn execute_http_body_equals_match_routes_on_match() {
	// Equals operator: body must be byte-identical to the compiled value.
	let sym = body_routing_graph(PredicateInst {
		path: FieldPath::HttpBody,
		op: CompiledOperator::Equals(CompiledValue::Bytes(Bytes::from_static(b"exact"))),
	});
	let req = http::Request::builder()
		.method("POST")
		.uri("/")
		.body(stream_body(b"exact"))
		.expect("build req");
	let (matched, missed) = run_body_routing(sym, req).await;
	assert_eq!(matched, 1, "on_match must fire when body equals compiled bytes");
	assert_eq!(missed, 0, "on_miss must not fire");
}

#[tokio::test]
async fn execute_http_body_prefix_match_routes_on_match() {
	// Prefix operator: body starts with the compiled prefix bytes.
	let sym = body_routing_graph(PredicateInst {
		path: FieldPath::HttpBody,
		op: CompiledOperator::Prefix(Bytes::from_static(b"hel")),
	});
	let req = http::Request::builder()
		.method("POST")
		.uri("/")
		.body(stream_body(b"hello world"))
		.expect("build req");
	let (matched, missed) = run_body_routing(sym, req).await;
	assert_eq!(matched, 1, "on_match must fire when body has expected prefix");
	assert_eq!(missed, 0, "on_miss must not fire");
}

#[tokio::test]
async fn execute_no_http_body_predicate_body_not_materialized() {
	// When no node carries collect_body_before, the request body must remain
	// Body::Stream at the middleware — buffering is pay-as-you-go.
	//
	// Graph: Middleware(body_capture, collect_body_before=None) → Terminate(Close)
	let captured = Arc::new(Mutex::new(None::<bool>));
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![],
		vec![SymbolicMiddlewareRef {
			name: Arc::from("body_probe"),
			args: Value::Null,
			kind: MiddlewareKind::L7Request,
			stateless: true,
			needs_body: false,
			on_error: None,
		}],
		vec![],
		vec![Terminator::Close],
	);

	let mut mw = MiddlewareFactories::new();
	{
		let cap = Arc::clone(&captured);
		mw.register("body_probe", MiddlewareKind::L7Request, move |_args| {
			Ok(MiddlewareInst::L7Request(Arc::new(BodyCapture(Arc::clone(&cap)))))
		});
	}
	let fetch = FetchFactories::new();
	let graph = FlowGraph::link(sym, &mw, &fetch).expect("link");
	let conn = make_conn("127.0.0.1:0");
	let sink = Arc::new(NullSink::new());

	let req: Request = http::Request::builder()
		.method("POST")
		.uri("/")
		.body(stream_body(b"unread payload"))
		.expect("build req");
	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(req)), &conn, &sink).await;
	assert!(result.is_ok(), "execute must succeed: {result:?}");
	assert_eq!(
		*captured.lock(),
		Some(false),
		"body must remain Body::Stream when no node sets collect_body_before"
	);
}
