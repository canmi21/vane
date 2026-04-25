//! Integration tests for `vane_engine::ListenerSet`.
//!
//! Covers the listener lifecycle described in
//! `spec/architecture/01-topology.md` § _Listener lifecycle_:
//!
//! * `Bind` — exponential-backoff bind with a `max_attempts` give-up branch
//!   (exercised via the `bind_with_retry_for_test` test helper).
//! * `Accept loop` — accepted TCP connections are routed into the executor
//!   and produce at least one `FlowLogKind::Trajectory` event per request.
//! * `Shutdown` — accept-loop cancel stops admitting new work; the soft
//!   drain waits up to `drain_timeout` for in-flight tasks to complete
//!   naturally before falling back to `force_cancel`.
//!
//! Tests build a minimal `SymbolicFlowGraph` with concrete `entries`,
//! linked through `FlowGraph::link`, and drive `ListenerSet::start` /
//! `ListenerSet::shutdown` against it. No configuration pipeline is
//! exercised.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use vane_core::{
	ConnContext, Decision, Error, FlowCtx, FlowGraphMeta, FlowLogEvent, FlowLogKind, FlowLogSink,
	L4BytesMiddleware, L4Conn, MiddlewareId, MiddlewareKind, Node, NodeId, SymbolicFlowGraph,
	SymbolicMiddlewareRef, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FlowGraph, MiddlewareInst};
use vane_engine::verbosity::VerbosityState;

// ---------------------------------------------------------------------------
// Recording sink: captures every emitted `FlowLogEvent` behind a `Mutex`. The
// listener's `start` takes `Arc<dyn FlowLogSink>` so the sink itself is
// shared between the test thread and the per-connection executor task.
// ---------------------------------------------------------------------------

struct RecordingSink {
	events: Mutex<Vec<FlowLogEvent>>,
}

impl RecordingSink {
	fn new() -> Self {
		Self { events: Mutex::new(Vec::new()) }
	}

	fn kinds(&self) -> Vec<FlowLogKind> {
		self.events.lock().iter().map(|e| e.kind).collect()
	}

	fn has_trajectory(&self) -> bool {
		self.events.lock().iter().any(|e| e.kind == FlowLogKind::Trajectory)
	}
}

impl FlowLogSink for RecordingSink {
	fn emit(&self, event: FlowLogEvent) {
		self.events.lock().push(event);
	}
}

// ---------------------------------------------------------------------------
// Free-port discovery. Bind ephemeral, take `local_addr()`, then drop the
// listener so the address is available to the listener-set under test.
// 01-topology.md § _Bind_ — the `entries` map needs a concrete `SocketAddr`,
// the listener crate doesn't accept "0".
// ---------------------------------------------------------------------------

async fn pick_port() -> SocketAddr {
	let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral for port pick");
	let addr = l.local_addr().expect("local_addr");
	drop(l);
	addr
}

// ---------------------------------------------------------------------------
// Graph builders. Each helper returns a linked `Arc<FlowGraph>` whose
// `symbolic().entries` already contains the listener address(es). The
// listener `start` reads that map and spawns one accept task per entry.
// ---------------------------------------------------------------------------

fn sample_meta() -> FlowGraphMeta {
	FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
	}
}

/// Trivial L4 entry graph: `entries[addr] -> Terminate(Close)`.
/// `Terminate` is L4-compatible per `is_l7_only_entry`'s policy
/// (Check / Upgrade / Terminate are all valid L4 entries).
fn close_only_graph(entries: HashMap<SocketAddr, NodeId>) -> Arc<FlowGraph> {
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![Node::Terminate(TerminatorId::new(0))],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![],
		terminators: vec![Terminator::Close],
		entries,
		meta: sample_meta(),
	});
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	FlowGraph::link(sym, &mw, &fetch).expect("link close-only graph")
}

/// `L4Bytes` middleware entry graph: `entries[addr] -> Middleware(SleepBytes)
/// -> Terminate(Close)`. The middleware sleeps `sleep_for` then returns
/// `Decision::Continue`. Used by the drain test to simulate an in-flight
/// task that finishes naturally before `drain_timeout` would expire.
fn sleep_bytes_graph(addr: SocketAddr, entry_node: NodeId, sleep_for: Duration) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, entry_node);
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![SymbolicMiddlewareRef {
			name: Arc::from("sleep_bytes"),
			args: Value::Null,
			kind: MiddlewareKind::L4Bytes,
			stateless: true,
			needs_body: false,
			on_error: None,
		}],
		fetches: vec![],
		terminators: vec![Terminator::Close],
		entries,
		meta: sample_meta(),
	});

	let mut mw = MiddlewareFactories::new();
	mw.register("sleep_bytes", MiddlewareKind::L4Bytes, move |_args| {
		Ok(MiddlewareInst::L4Bytes(Arc::new(SleepBytes { delay: sleep_for })))
	});
	let fetch = FetchFactories::new();
	FlowGraph::link(sym, &mw, &fetch).expect("link sleep-bytes graph")
}

/// Sleeps `delay`, then returns `Decision::Continue`. Implements the
/// `L4BytesMiddleware` trait declared in `vane_core::middleware`.
struct SleepBytes {
	delay: Duration,
}

#[async_trait]
impl L4BytesMiddleware for SleepBytes {
	async fn run(
		&self,
		_l4: &mut L4Conn,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		tokio::time::sleep(self.delay).await;
		Ok(Decision::Continue)
	}
}

// ---------------------------------------------------------------------------
// 1. listener_accepts_tcp_and_routes_to_executor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listener_accepts_tcp_and_routes_to_executor() {
	// 01-topology.md § _Accept loop_: each accepted connection spawns a
	// per-connection task that drives the executor against the captured
	// `Arc<FlowGraph>`. The executor must emit at least one
	// `FlowLogKind::Trajectory` event into the listener-supplied sink.
	let addr = pick_port().await;
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));
	let graph = close_only_graph(entries);

	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	let set = ListenerSet::new();
	set.start(Arc::clone(&graph), Arc::clone(&verbosity), sink_dyn);
	assert!(set.is_running(&addr), "start must register a running listener for the entry addr");
	assert_eq!(set.len(), 1, "exactly one entry → one running listener");

	// Give the accept loop a moment to bind before the client connects.
	tokio::time::sleep(Duration::from_millis(50)).await;
	let client = tokio::net::TcpStream::connect(addr).await.expect("client connects to listener");
	drop(client);

	// Yield to the runtime so the accept loop polls `listener.accept()` and
	// spawns the per-connection task BEFORE shutdown fires `accept_cancel`.
	// With a `biased` select, a connected-but-unaccepted client would
	// otherwise lose to the cancel branch and never reach the executor.
	tokio::time::sleep(Duration::from_millis(50)).await;

	// Soft-drain shutdown lets the executor finish naturally — the Close
	// terminator path is microseconds, so the drain wait won't elapse.
	set.shutdown(Duration::from_secs(2)).await;

	assert!(
		sink.has_trajectory(),
		"per-request Trajectory event must land in the listener sink; saw {:?}",
		sink.kinds(),
	);
}

// ---------------------------------------------------------------------------
// 3. listener_bind_giving_up_after_max_attempts_logs_and_exits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listener_bind_giving_up_after_max_attempts_logs_and_exits() {
	// 01-topology.md § _Bind_: bind retries on failure with exponential
	// backoff up to `max_attempts`, then gives up. Hold a real listener on
	// the address so every retry observes EADDRINUSE; the helper returns
	// `None` once the cap is reached.
	let occupier =
		tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind occupier listener");
	let addr = occupier.local_addr().expect("local_addr");

	let result = vane_engine::listener::bind_with_retry_for_test(addr, 2).await;
	assert!(result.is_none(), "bind_with_retry_for_test must give up after max_attempts");

	drop(occupier);
}

// ---------------------------------------------------------------------------
// 4. listener_drains_in_flight_within_timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listener_drains_in_flight_within_timeout() {
	// 01-topology.md § _Listener lifecycle_ step 3: `accept_cancel` stops
	// new connections; in-flight tasks get up to `drain_timeout` to finish
	// naturally before `force_cancel` fires. With a middleware that sleeps
	// 200ms and a 2s drain budget, shutdown must complete well under the
	// budget — the executor finishes long before the timeout expires.
	let addr = pick_port().await;
	let graph = sleep_bytes_graph(addr, NodeId::new(0), Duration::from_millis(200));

	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	let set = ListenerSet::new();
	set.start(Arc::clone(&graph), Arc::clone(&verbosity), sink_dyn);

	// Wait briefly so the accept loop binds, then connect.
	tokio::time::sleep(Duration::from_millis(50)).await;
	let client = tokio::net::TcpStream::connect(addr).await.expect("client connects to listener");

	// Sleep ~50ms so the middleware is mid-sleep when shutdown fires.
	tokio::time::sleep(Duration::from_millis(50)).await;

	let started = Instant::now();
	set.shutdown(Duration::from_secs(2)).await;
	let elapsed = started.elapsed();

	drop(client);

	assert!(
		elapsed < Duration::from_millis(1500),
		"shutdown must complete via natural drain well under the 2s budget; elapsed = {elapsed:?}",
	);
	assert!(
		sink.has_trajectory(),
		"in-flight task must complete and emit a Trajectory event; saw {:?}",
		sink.kinds(),
	);
}

// ---------------------------------------------------------------------------
// 5. listener_set_starts_multiple_entries_independently
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listener_set_starts_multiple_entries_independently() {
	// 01-topology.md § _Listener lifecycle_: listeners are independent
	// tokio tasks per `(transport, address)` pair. A graph with two
	// entries spawns two listeners; both report `is_running` and `len`
	// reflects the count.
	let addr1 = pick_port().await;
	let addr2 = pick_port().await;
	assert_ne!(addr1, addr2, "pick_port must produce distinct addrs");

	// Both entries point at the same `Terminate(Close)` node; the listener
	// only cares that the entry is L4-compatible.
	let mut entries = HashMap::new();
	entries.insert(addr1, NodeId::new(0));
	entries.insert(addr2, NodeId::new(0));
	let graph = close_only_graph(entries);

	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	let set = ListenerSet::new();
	set.start(Arc::clone(&graph), Arc::clone(&verbosity), sink_dyn);

	assert_eq!(set.len(), 2, "two entries → two running listeners");
	assert!(set.is_running(&addr1), "addr1 listener must be registered");
	assert!(set.is_running(&addr2), "addr2 listener must be registered");
	assert!(!set.is_empty(), "set is non-empty when entries are running");

	// Give both accept loops time to bind before connecting.
	tokio::time::sleep(Duration::from_millis(50)).await;
	let c1 = tokio::net::TcpStream::connect(addr1).await.expect("connect addr1");
	let c2 = tokio::net::TcpStream::connect(addr2).await.expect("connect addr2");
	drop(c1);
	drop(c2);

	set.shutdown(Duration::from_secs(2)).await;
}

// ---------------------------------------------------------------------------
// 6. listener_shutdown_idempotent_or_after_empty_start
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listener_shutdown_idempotent_or_after_empty_start() {
	// 01-topology.md § _Listener lifecycle_: a `ListenerSet` that was
	// never started (or was started with an empty `entries` map) must
	// shutdown cleanly without panic. `shutdown` consumes `self`, so
	// "double shutdown" is a compile error — this test validates the
	// empty-set drain path returns promptly.
	let set = ListenerSet::new();
	assert!(set.is_empty(), "fresh ListenerSet has no listeners");
	assert_eq!(set.len(), 0);

	let graph = close_only_graph(HashMap::new());
	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	set.start(Arc::clone(&graph), Arc::clone(&verbosity), sink_dyn);
	assert!(set.is_empty(), "no entries → no listeners spawned");

	let started = Instant::now();
	set.shutdown(Duration::from_millis(100)).await;
	let elapsed = started.elapsed();
	assert!(
		elapsed < Duration::from_millis(500),
		"empty-set shutdown returns promptly; elapsed = {elapsed:?}",
	);
}
