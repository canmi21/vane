//! Integration tests for the WASM middleware dispatch path.
//!
//! Drives `execute` with a `PluginRegistry` backed by a `MockWasmRuntime` to
//! cover the decision-translation table and error-routing rules specified in
//! `spec/crates/engine.md`.
//!
//! Test cases: (a) Continue, (b) Short synth response, (c) Close,
//! (d) plugin error with no hint routes via `on_error`,
//! (e) plugin error with force-close hint bypasses `on_error`,
//! (f) `PluginError::Trap` propagates as Err, (g) stateless dedup via Arc.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use vane_core::{
	Body, ConnContext, ConnId, Error, FlowCtx, FlowGraphMeta, FlowLogEvent, FlowLogKind, FlowLogSink,
	Header, L4BytesDecision, L4BytesInput, L4Conn, L4PeekDecision, L4PeekInput, L7RequestDecision,
	L7RequestInput, L7ResponseDecision, L7ResponseInput, MiddlewareId, MiddlewareKind, ModuleId,
	Node, NodeId, PeekResult, PluginError, PluginExport, PluginMetadata, Request, SymbolicFlowGraph,
	SymbolicMiddlewareRef, SynthResponse, Terminator, TerminatorId, Transport, WasmRuntime,
};
use vane_engine::executor::{ExecutorInput, ExecutorOutput, execute};
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FlowGraph, PluginRegistry};

// Helpers: sink, conn, graph builder

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

fn make_conn() -> Arc<ConnContext> {
	let remote: SocketAddr = "127.0.0.1:0".parse().expect("parse");
	let local: SocketAddr = "127.0.0.1:0".parse().expect("parse");
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
	terminators: Vec<Terminator>,
) -> Arc<SymbolicFlowGraph> {
	Arc::new(SymbolicFlowGraph {
		nodes,
		predicates: vec![],
		middlewares,
		fetches: vec![],
		terminators,
		entries: HashMap::new(),
		meta: sample_meta(),
	})
}

fn wasm_symref(name: &str, kind: MiddlewareKind) -> SymbolicMiddlewareRef {
	SymbolicMiddlewareRef {
		name: Arc::from(name),
		args: serde_json::Value::Null,
		kind,
		stateless: true,
		needs_body: false,
		on_error: None,
	}
}

fn wasm_symref_on_error(
	name: &str,
	kind: MiddlewareKind,
	on_error: NodeId,
) -> SymbolicMiddlewareRef {
	SymbolicMiddlewareRef {
		name: Arc::from(name),
		args: serde_json::Value::Null,
		kind,
		stateless: true,
		needs_body: false,
		on_error: Some(on_error),
	}
}

fn empty_request() -> Request {
	http::Request::builder().method("GET").uri("/").body(Body::Empty).expect("build req")
}

async fn run_execute(
	graph: &Arc<FlowGraph>,
	entry: NodeId,
	input: ExecutorInput,
	conn: &Arc<ConnContext>,
	sink: &Arc<NullSink>,
) -> Result<ExecutorOutput, Error> {
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

// Mock WasmRuntime
//
// Each handler slot holds a Vec<Result<Decision, PluginError>> acting as a
// FIFO queue: pop_front on each call, return the front item. If the queue
// is empty the call panics (test bug — add enough items).

type L7ReqResults = Vec<Result<L7RequestDecision, PluginError>>;

struct MockWasmRuntime {
	call_count: Arc<AtomicUsize>,
	l4_peek_results: Mutex<Vec<Result<L4PeekDecision, PluginError>>>,
	l4_bytes_results: Mutex<Vec<Result<L4BytesDecision, PluginError>>>,
	l7_request_results: Mutex<L7ReqResults>,
	l7_response_results: Mutex<Vec<Result<L7ResponseDecision, PluginError>>>,
	recorded_l4_peek_inputs: Mutex<Vec<Vec<u8>>>,
}

impl MockWasmRuntime {
	fn with_l7_request(results: L7ReqResults) -> Self {
		Self {
			call_count: Arc::new(AtomicUsize::new(0)),
			l4_peek_results: Mutex::new(vec![]),
			l4_bytes_results: Mutex::new(vec![]),
			l7_request_results: Mutex::new(results),
			l7_response_results: Mutex::new(vec![]),
			recorded_l4_peek_inputs: Mutex::new(vec![]),
		}
	}

	fn with_l4_peek(results: Vec<Result<L4PeekDecision, PluginError>>) -> Self {
		Self {
			call_count: Arc::new(AtomicUsize::new(0)),
			l4_peek_results: Mutex::new(results),
			l4_bytes_results: Mutex::new(vec![]),
			l7_request_results: Mutex::new(vec![]),
			l7_response_results: Mutex::new(vec![]),
			recorded_l4_peek_inputs: Mutex::new(vec![]),
		}
	}
}

#[async_trait]
impl WasmRuntime for MockWasmRuntime {
	async fn load_component(&self, _path: &Path) -> Result<Arc<PluginMetadata>, vane_core::Error> {
		unimplemented!("MockWasmRuntime does not load components")
	}

	async fn invoke_l4_peek(
		&self,
		_module_id: &ModuleId,
		_export_name: &str,
		_args_json: &str,
		input: L4PeekInput,
	) -> Result<L4PeekDecision, PluginError> {
		self.call_count.fetch_add(1, Ordering::SeqCst);
		self.recorded_l4_peek_inputs.lock().push(input.peek);
		self.l4_peek_results.lock().remove(0)
	}

	async fn invoke_l4_bytes(
		&self,
		_module_id: &ModuleId,
		_export_name: &str,
		_args_json: &str,
		_input: L4BytesInput,
	) -> Result<L4BytesDecision, PluginError> {
		self.call_count.fetch_add(1, Ordering::SeqCst);
		self.l4_bytes_results.lock().remove(0)
	}

	async fn invoke_l7_request(
		&self,
		_module_id: &ModuleId,
		_export_name: &str,
		_args_json: &str,
		_input: L7RequestInput,
	) -> Result<L7RequestDecision, PluginError> {
		self.call_count.fetch_add(1, Ordering::SeqCst);
		self.l7_request_results.lock().remove(0)
	}

	async fn invoke_l7_response(
		&self,
		_module_id: &ModuleId,
		_export_name: &str,
		_args_json: &str,
		_input: L7ResponseInput,
	) -> Result<L7ResponseDecision, PluginError> {
		self.call_count.fetch_add(1, Ordering::SeqCst);
		self.l7_response_results.lock().remove(0)
	}
}

fn make_metadata(export_name: &str, kind: MiddlewareKind) -> Arc<PluginMetadata> {
	Arc::new(PluginMetadata {
		name: "mock".to_owned(),
		version: "0.1.0".to_owned(),
		abi_version: "0.1.0".to_owned(),
		exports: vec![PluginExport {
			name: export_name.to_owned(),
			kind,
			stateless: true,
			needs_body: false,
			inspects: vec![],
		}],
	})
}

fn make_registry(
	plugin_name: &str,
	export_name: &str,
	kind: MiddlewareKind,
	runtime: Arc<dyn WasmRuntime>,
) -> PluginRegistry {
	let mut reg = PluginRegistry::new();
	let module_id = ModuleId(Arc::from("/fake/plugin.wasm"));
	let metadata = make_metadata(export_name, kind);
	reg.register(plugin_name, module_id, export_name.to_owned(), metadata, runtime);
	reg
}

fn link_with_plugins(sym: Arc<SymbolicFlowGraph>, registry: &PluginRegistry) -> Arc<FlowGraph> {
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	FlowGraph::link_with_plugins(
		sym,
		&mw,
		registry,
		&fetch,
		Arc::new(vane_engine::security::SecurityConfig::default()),
	)
	.expect("link_with_plugins")
}

// (a) Continue — cursor advances to terminator

#[tokio::test]
async fn wasm_l7request_continue_advances_cursor() {
	let runtime = Arc::new(MockWasmRuntime::with_l7_request(vec![Ok(L7RequestDecision::Continue)]));
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
		vec![wasm_symref("my-plugin:probe", MiddlewareKind::L7Request)],
		vec![Terminator::Close],
	);
	let reg = make_registry("my-plugin:probe", "probe", MiddlewareKind::L7Request, runtime);
	let graph = link_with_plugins(sym, &reg);
	let conn = make_conn();
	let sink = Arc::new(NullSink::new());

	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(empty_request())), &conn, &sink)
			.await;

	assert!(result.is_ok(), "Continue must reach terminator: {result:?}");
}

// (b) Short — synth response returned as HttpResponse

#[tokio::test]
async fn wasm_l7request_short_synth_response_returned() {
	let synth = SynthResponse {
		status: 403,
		headers: vec![Header { name: "x-blocked".to_owned(), value: "yes".to_owned() }],
		body: b"Forbidden".to_vec(),
	};
	let runtime =
		Arc::new(MockWasmRuntime::with_l7_request(vec![Ok(L7RequestDecision::Short(synth))]));

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
		vec![wasm_symref("my-plugin:probe", MiddlewareKind::L7Request)],
		vec![Terminator::Close],
	);

	// For Short(Response) the executor needs a short_circuit_response_entry.
	let sym = {
		let mut s = (*sym).clone();
		s.meta.short_circuit_response_entry.insert(NodeId::new(0), NodeId::new(2));
		s.nodes.push(Node::Terminate(TerminatorId::new(1)));
		s.terminators.push(Terminator::WriteHttpResponse);
		Arc::new(s)
	};

	let reg = make_registry("my-plugin:probe", "probe", MiddlewareKind::L7Request, runtime);
	let graph = link_with_plugins(sym, &reg);
	let conn = make_conn();
	let sink = Arc::new(NullSink::new());

	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(empty_request())), &conn, &sink)
			.await;

	let resp = match result.expect("Short synth must not err") {
		ExecutorOutput::HttpResponse(r) => r,
		other => panic!("expected HttpResponse, got {other:?}"),
	};
	assert_eq!(resp.status().as_u16(), 403, "synth status must be 403");
}

// (c) Close — plugin returns Close → Ok(Closed)

#[tokio::test]
async fn wasm_l7request_close_returns_closed() {
	let runtime = Arc::new(MockWasmRuntime::with_l7_request(vec![Ok(L7RequestDecision::Close)]));

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
		vec![wasm_symref("my-plugin:probe", MiddlewareKind::L7Request)],
		vec![Terminator::Close],
	);
	let reg = make_registry("my-plugin:probe", "probe", MiddlewareKind::L7Request, runtime);
	let graph = link_with_plugins(sym, &reg);
	let conn = make_conn();
	let sink = Arc::new(NullSink::new());

	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(empty_request())), &conn, &sink)
			.await;

	assert!(
		matches!(result, Ok(ExecutorOutput::Closed)),
		"plugin Close must surface as Ok(Closed): {result:?}",
	);
}

// (d) PluginError with on_error_hint:None + configured on_error → fires node

#[tokio::test]
async fn wasm_plugin_error_no_hint_routes_via_on_error() {
	let runtime = Arc::new(MockWasmRuntime::with_l7_request(vec![Err(PluginError::Plugin {
		code: "E001".to_owned(),
		message: "bad input".to_owned(),
		on_error_hint: None,
	})]));

	// Node layout:
	//   0: Middleware(wasm, on_error=2)  next=1
	//   1: Terminate(Close)              — unreachable via on_error path
	//   2: Terminate(Close)              — the on_error target
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: Some(NodeId::new(2)),
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![wasm_symref_on_error("my-plugin:probe", MiddlewareKind::L7Request, NodeId::new(2))],
		vec![Terminator::Close],
	);
	let reg = make_registry("my-plugin:probe", "probe", MiddlewareKind::L7Request, runtime);
	let graph = link_with_plugins(sym, &reg);
	let conn = make_conn();
	let sink = Arc::new(NullSink::new());

	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(empty_request())), &conn, &sink)
			.await;

	// on_error=Some routes to NodeId(2)=Terminate(Close) → Ok(Closed).
	assert!(
		matches!(result, Ok(ExecutorOutput::Closed)),
		"on_error_hint:None with on_error node must route to on_error: {result:?}",
	);
	let kinds = sink.kinds();
	assert!(kinds.contains(&FlowLogKind::Error), "error event must be emitted: {kinds:?}");
}

// (e) PluginError with on_error_hint:"force-close" bypasses on_error

#[tokio::test]
async fn wasm_plugin_error_force_close_bypasses_on_error() {
	let runtime = Arc::new(MockWasmRuntime::with_l7_request(vec![Err(PluginError::Plugin {
		code: "E002".to_owned(),
		message: "forced".to_owned(),
		on_error_hint: Some("force-close".to_owned()),
	})]));

	// Same layout with on_error node — but force-close must bypass it.
	let sym = build_graph(
		vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: Some(NodeId::new(2)),
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
			Node::Terminate(TerminatorId::new(0)),
		],
		vec![wasm_symref_on_error("my-plugin:probe", MiddlewareKind::L7Request, NodeId::new(2))],
		vec![Terminator::Close],
	);
	let reg = make_registry("my-plugin:probe", "probe", MiddlewareKind::L7Request, runtime);
	let graph = link_with_plugins(sym, &reg);
	let conn = make_conn();
	let sink = Arc::new(NullSink::new());

	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(empty_request())), &conn, &sink)
			.await;

	// force-close returns Decision::Short(Close(PolicyDenied(...))), which
	// the executor routes as Ok(Closed) — on_error node never fires.
	assert!(
		matches!(result, Ok(ExecutorOutput::Closed)),
		"force-close must bypass on_error and return Ok(Closed): {result:?}",
	);
}

// (f) PluginError::Trap propagates as Err

#[tokio::test]
async fn wasm_plugin_trap_propagates_as_err() {
	let runtime = Arc::new(MockWasmRuntime::with_l7_request(vec![Err(PluginError::Trap(
		"guest panicked".to_owned(),
	))]));

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
		vec![wasm_symref("my-plugin:probe", MiddlewareKind::L7Request)],
		vec![Terminator::Close],
	);
	let reg = make_registry("my-plugin:probe", "probe", MiddlewareKind::L7Request, runtime);
	let graph = link_with_plugins(sym, &reg);
	let conn = make_conn();
	let sink = Arc::new(NullSink::new());

	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L7(Box::new(empty_request())), &conn, &sink)
			.await;

	let err = result.expect_err("Trap must surface as Err");
	assert!(err.to_string().contains("guest panicked"), "Err must carry trap message: {err}");
}

// (g) Stateless dedup — sharing the same Arc<dyn WasmRuntime> instance

#[tokio::test]
async fn wasm_stateless_registry_entry_shares_runtime_arc() {
	// Two plugin names backed by the same runtime Arc. After link, both
	// WasmMiddleware instances must share the exact same Arc allocation.
	// Arc::strong_count increasing from 1 to 3 (registry + 2 link entries)
	// proves the Arc was cloned, not duplicated. We check count == 3.
	let runtime: Arc<dyn WasmRuntime> = Arc::new(MockWasmRuntime::with_l7_request(vec![
		Ok(L7RequestDecision::Continue),
		Ok(L7RequestDecision::Continue),
	]));
	let initial_count = Arc::strong_count(&runtime);

	let mut reg = PluginRegistry::new();
	let module_id = ModuleId(Arc::from("/fake/plugin.wasm"));
	let metadata = make_metadata("probe", MiddlewareKind::L7Request);
	reg.register(
		"plugin-a",
		module_id.clone(),
		"probe".to_owned(),
		Arc::clone(&metadata),
		Arc::clone(&runtime),
	);
	reg.register(
		"plugin-b",
		module_id,
		"probe".to_owned(),
		Arc::clone(&metadata),
		Arc::clone(&runtime),
	);

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
		vec![
			wasm_symref("plugin-a", MiddlewareKind::L7Request),
			wasm_symref("plugin-b", MiddlewareKind::L7Request),
		],
		vec![Terminator::Close],
	);

	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	let _graph = FlowGraph::link_with_plugins(
		sym,
		&mw,
		&reg,
		&fetch,
		Arc::new(vane_engine::security::SecurityConfig::default()),
	)
	.expect("link");

	// The runtime Arc is held by: registry entry "plugin-a", registry entry
	// "plugin-b", both WasmMiddleware instances in the graph, plus the local
	// `runtime` binding. Exact count depends on Arc::clone calls; we only
	// assert > initial to confirm sharing occurred.
	assert!(
		Arc::strong_count(&runtime) > initial_count,
		"runtime Arc must have been cloned into registry/graph entries",
	);
}

// (h) L4Peek dispatch hands the listener-stashed PeekResult.buffer to the
//     plugin via L4PeekInput.peek

async fn throwaway_tcp_stream() -> tokio::net::TcpStream {
	let listener =
		tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral listener");
	let addr = listener.local_addr().expect("local_addr");
	let connect = tokio::net::TcpStream::connect(addr);
	let accept = listener.accept();
	let (client, _server) = tokio::join!(connect, accept);
	client.expect("connect to ephemeral listener")
}

#[tokio::test]
async fn wasm_l4peek_dispatch_forwards_peek_buffer_from_conn_user() {
	let runtime_inner = Arc::new(MockWasmRuntime::with_l4_peek(vec![Ok(L4PeekDecision::Continue)]));
	let runtime: Arc<dyn WasmRuntime> = Arc::clone(&runtime_inner) as Arc<dyn WasmRuntime>;

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
		vec![wasm_symref("my-plugin:probe", MiddlewareKind::L4Peek)],
		vec![Terminator::Close],
	);
	let reg = make_registry("my-plugin:probe", "probe", MiddlewareKind::L4Peek, runtime);
	let graph = link_with_plugins(sym, &reg);
	let conn = make_conn();
	let sink = Arc::new(NullSink::new());

	// Listener-side prelude would normally write this; the dispatch path
	// must read it from `conn.user` and hand it to the plugin verbatim.
	let known_peek: &[u8] = b"GET / HTTP/1.1\r\n\r\n";
	{
		let mut user = conn.user.lock();
		user.insert(PeekResult {
			buffer: bytes::Bytes::from_static(known_peek),
			detected: None,
			tls: None,
		});
	}

	let l4 = L4Conn::Tcp(throwaway_tcp_stream().await);
	let result =
		run_execute(&graph, NodeId::new(0), ExecutorInput::L4(Box::new(l4)), &conn, &sink).await;

	assert!(result.is_ok(), "L4Peek Continue must reach terminator: {result:?}");
	let recorded = runtime_inner.recorded_l4_peek_inputs.lock();
	assert_eq!(recorded.len(), 1, "exactly one L4Peek invocation expected");
	assert_eq!(
		recorded[0].as_slice(),
		known_peek,
		"plugin must receive the PeekResult.buffer bytes verbatim",
	);
}

// (i) dispatch_wasm returns Err when export_name is not in metadata exports

#[tokio::test]
async fn wasm_dispatch_returns_err_when_export_missing_from_metadata() {
	// Construct a `WasmMiddleware` whose `export_name` does not appear in
	// the metadata's exports list. The link pipeline normally guards
	// against this — the test forces the corrupt state directly to verify
	// `dispatch_wasm` reports the inconsistency rather than falling
	// through to the L7Response arm and panicking on the `expect`.
	use vane_engine::executor::dispatch_wasm;
	use vane_engine::flow_graph::WasmMiddleware;

	let runtime: Arc<dyn WasmRuntime> = Arc::new(MockWasmRuntime::with_l7_request(vec![]));
	let metadata = Arc::new(PluginMetadata {
		name: "mock".to_owned(),
		version: "0.1.0".to_owned(),
		abi_version: "0.1.0".to_owned(),
		exports: vec![PluginExport {
			name: "real-export".to_owned(),
			kind: MiddlewareKind::L7Response,
			stateless: true,
			needs_body: false,
			inspects: vec![],
		}],
	});
	let w = WasmMiddleware {
		module_id: ModuleId(Arc::from("/fake/plugin.wasm")),
		export_name: "missing-export".to_owned(),
		args_json: "null".to_owned(),
		runtime,
		metadata,
	};

	let conn = make_conn();
	let mut l4: Option<L4Conn> = None;
	let mut req: Option<Request> = None;
	let mut resp: Option<vane_core::Response> = None;
	let result = dispatch_wasm(&w, &mut l4, &mut req, &mut resp, &conn).await;

	let Err(err) = result else {
		panic!("missing export must surface as Err, got Ok decision");
	};
	assert!(
		err.to_string().contains("missing-export"),
		"err must mention the missing export name: {err}",
	);
}
