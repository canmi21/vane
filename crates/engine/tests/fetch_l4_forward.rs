//! Integration tests for `vane_engine::fetch::l4_forward`.
//!
//! Covers the L4 forward Fetch contract described in
//! `spec/crates/engine.md` § _Concrete fetches_ and
//! `spec/crates/engine.md` § _Concrete fetches_ /
//! _`Tunnel` + `ByteTunnel` terminator_:
//!
//! * On `L4Conn::Tcp`, the Fetch dials the configured upstream and hands
//!   the executor a `Tunnel` paired with the inbound socket.
//! * `Terminator::ByteTunnel` drives `tokio::io::copy_bidirectional`,
//!   relaying bytes in both directions and propagating EOF cleanly.
//! * Dial failure surfaces as `Error::upstream(Unreachable)`, ending the
//!   connection without producing a response.
//! * The factory rejects missing-or-empty `upstream` args.
//!
//! Tests build a minimal `SymbolicFlowGraph` whose entry is a
//! `Node::Fetch` referencing the registered `L4Forward` factory, link it
//! through `FlowGraph::link`, and drive `ListenerSet::start` against a
//! freshly bound port. The proxy stands between a client task and a
//! tokio echo server, which lets us exercise byte semantics end-to-end
//! without poking at executor internals.
//!
//! Args shape mirrors `spec/crates/core.md` § _Compile pipeline_
//! (`{ "upstream": "host:port" }`).

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogKind, FlowLogSink, FlowTrajectory, Node,
	NodeId, SymbolicFetchRef, SymbolicFlowGraph, Terminator, TerminatorId, TerminatorOutcomeKind,
	TrajectoryOutcome,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FactoryError, FetchFactories, MiddlewareFactories};
use vane_engine::fetch::l4_forward;
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// Recording sink: captures every emitted `FlowLogEvent` behind a `Mutex`.
// Mirrors the helper in `tests/listener.rs`. Tests inspect the captured
// events to assert the per-request `Trajectory` event lands with the
// expected `TrajectoryOutcome`.

struct RecordingSink {
	events: Mutex<Vec<FlowLogEvent>>,
}

impl RecordingSink {
	fn new() -> Self {
		Self { events: Mutex::new(Vec::new()) }
	}

	fn snapshot(&self) -> Vec<FlowLogEvent> {
		self.events.lock().clone()
	}

	fn kinds(&self) -> Vec<FlowLogKind> {
		self.events.lock().iter().map(|e| e.kind).collect()
	}
}

impl FlowLogSink for RecordingSink {
	fn emit(&self, event: FlowLogEvent) {
		self.events.lock().push(event);
	}
}

// Free-port discovery. Bind ephemeral, take `local_addr()`, then drop the
// listener so the address is available again. spec/topology.md § _Bind_:
// the `entries` map must carry a concrete `SocketAddr`.

async fn pick_port() -> SocketAddr {
	let l = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral for port pick");
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

		listener_transports: std::collections::BTreeMap::new(),
		annotations: Vec::new(),
	}
}

/// Build a 2-node L4 forward graph rooted at the listener address:
///
/// ```text
///   0: Fetch { id: 0, next_response: None, next_tunnel: 1, ... }
///   1: Terminate(ByteTunnel)
/// ```
///
/// Per `spec/flow-model.md` § _Executor_ and `spec/crates/engine.md`
/// § _Concrete fetches_, an L4 path through `L4ForwardFetch` must end in
/// `Terminator::ByteTunnel`. The fetch is registered through
/// `vane_engine::fetch::l4_forward::register` so the factory lookup at
/// link time succeeds.
fn make_proxy_graph(listen: SocketAddr, upstream: &str) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Fetch {
				id: FetchId::new(0),
				next_response: None,
				next_tunnel: Some(NodeId::new(1)),
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef {
			kind: FetchKind::L4Forward,
			args: serde_json::json!({ "upstream": upstream }),
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		terminators: vec![Terminator::ByteTunnel],
		entries,
		meta: sample_meta(),
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	l4_forward::register(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link l4_forward graph")
}

// Echo upstream: accept loop that reads everything a client sends and writes
// it straight back. Caller controls the lifecycle by holding the returned
// addr and dropping the spawned task at end-of-test. The accept loop runs
// until `tokio::test`'s runtime tears it down at scope exit.

async fn spawn_echo_upstream() -> SocketAddr {
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind echo upstream");
	let addr = listener.local_addr().expect("upstream local_addr");
	tokio::spawn(async move {
		loop {
			let Ok((mut sock, _)) = listener.accept().await else { return };
			tokio::spawn(async move {
				let mut buf = [0u8; 1024];
				loop {
					match sock.read(&mut buf).await {
						Ok(0) | Err(_) => return,
						Ok(n) => {
							if sock.write_all(&buf[..n]).await.is_err() {
								return;
							}
						}
					}
				}
			});
		}
	});
	addr
}

/// Pull the deserialized `FlowTrajectory` out of the first
/// `FlowLogKind::Trajectory` event in the sink, panicking if absent. Per
/// `flow_log.rs`, Trajectory events carry a serialized `FlowTrajectory`
/// in the `data` field — every request emits exactly one regardless of
/// verbosity.
fn first_trajectory(sink: &RecordingSink) -> FlowTrajectory {
	let events = sink.snapshot();
	let event = events
		.iter()
		.find(|e| e.kind == FlowLogKind::Trajectory)
		.unwrap_or_else(|| panic!("no Trajectory event in sink; saw {:?}", sink.kinds()));
	let data = event.data.clone().expect("Trajectory event must carry data payload");
	serde_json::from_value::<FlowTrajectory>(data)
		.expect("Trajectory data deserialises to FlowTrajectory")
}

// 1. l4_forward_echoes_bytes_through_upstream

#[tokio::test]
async fn l4_forward_echoes_bytes_through_upstream() {
	// spec/crates/engine.md § _Concrete fetches_: TCP path uses `copy_bidirectional` to
	// shovel bytes between the inbound client socket and the freshly
	// dialed upstream. Round-tripping `b"ping"` through the proxy and
	// back via an echo server confirms both directions copy cleanly.
	let upstream_addr = spawn_echo_upstream().await;
	let proxy_addr = pick_port().await;
	let graph = make_proxy_graph(proxy_addr, &upstream_addr.to_string());

	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink_dyn);

	// Listener tests cap wait at ~50ms before the first connect — same here.
	tokio::time::sleep(Duration::from_millis(50)).await;

	let mut client = TcpStream::connect(proxy_addr).await.expect("connect proxy");
	client.write_all(b"ping").await.expect("write payload");
	// Half-close write so the upstream's read loop sees EOF and the
	// `copy_bidirectional` upstream→client direction can drain to FIN.
	client.shutdown().await.expect("client write shutdown");

	let mut received = Vec::new();
	client.read_to_end(&mut received).await.expect("read echoed payload");
	assert_eq!(received, b"ping", "echo upstream must round-trip the payload byte-for-byte");

	// Yield so the per-connection task has a chance to record its
	// Trajectory event before shutdown — see commit 9c10b2f4 ("yield to
	// accept loop before listener shutdown").
	tokio::time::sleep(Duration::from_millis(50)).await;
	set.shutdown(Duration::from_millis(500)).await;
}

// 2. l4_forward_propagates_upstream_eof_to_client

#[tokio::test]
async fn l4_forward_propagates_upstream_eof_to_client() {
	// spec/crates/engine.md § _Concrete fetches_ + spec/crates/engine.md § _Fetch_: when the
	// upstream FINs, `copy_bidirectional` propagates the EOF to the
	// client side, the tunnel terminates Ok, and the client sees a clean
	// `Ok(0)` from `read_to_end`.
	let upstream_listener =
		TcpListener::bind("127.0.0.1:0").await.expect("bind eof-on-read upstream");
	let upstream_addr = upstream_listener.local_addr().expect("upstream local_addr");
	tokio::spawn(async move {
		loop {
			let Ok((mut sock, _)) = upstream_listener.accept().await else { return };
			tokio::spawn(async move {
				let mut buf = [0u8; 1];
				let _ = sock.read(&mut buf).await;
				// Drop the socket — both halves close, propagating EOF
				// back through the tunnel to the client.
				drop(sock);
			});
		}
	});

	let proxy_addr = pick_port().await;
	let graph = make_proxy_graph(proxy_addr, &upstream_addr.to_string());

	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink_dyn);
	tokio::time::sleep(Duration::from_millis(50)).await;

	let mut client = TcpStream::connect(proxy_addr).await.expect("connect proxy");
	client.write_all(b"x").await.expect("write one byte");
	let mut buf = Vec::new();
	let n = client.read_to_end(&mut buf).await.expect("read_to_end on client side");
	assert_eq!(n, 0, "upstream EOF must propagate as a clean client EOF, not an io error");

	tokio::time::sleep(Duration::from_millis(50)).await;
	set.shutdown(Duration::from_millis(500)).await;
}

// 3. l4_forward_unreachable_upstream_surfaces_as_walker_err

#[tokio::test]
async fn l4_forward_unreachable_upstream_surfaces_as_walker_err() {
	// spec/crates/engine.md § _Concrete fetches_: dial failure is typed as
	// `Error::upstream(Unreachable)` and propagates through the
	// executor, which finalises a `TrajectoryOutcome::Error { .. }` and
	// emits a Trajectory event. The client connection terminates without
	// any bytes being relayed.
	//
	// Picking an unbound address: `pick_port()` binds + drops, leaving a
	// brief reuse window on darwin. The window is small enough in
	// practice for the test to remain stable; if a future test run
	// flakes here, swapping in `127.0.0.1:1` (privileged → refused) is
	// the documented fallback.
	let unreachable_addr = pick_port().await;
	let proxy_addr = pick_port().await;
	let graph = make_proxy_graph(proxy_addr, &unreachable_addr.to_string());

	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink_dyn);
	tokio::time::sleep(Duration::from_millis(50)).await;

	let mut client = TcpStream::connect(proxy_addr).await.expect("connect proxy");
	let mut buf = Vec::new();
	// Either an io error (RST) or a clean 0-byte EOF is acceptable —
	// what matters is "no bytes relayed" + Trajectory error in the sink.
	// `Err(_)` (RST) is also acceptable; both signal "no bytes relayed".
	if let Ok(n) = client.read_to_end(&mut buf).await {
		assert_eq!(n, 0, "unreachable upstream must yield no bytes; got {n}");
	}

	// Allow the per-connection task to finalize its trajectory.
	tokio::time::sleep(Duration::from_millis(100)).await;
	set.shutdown(Duration::from_millis(500)).await;

	let traj = first_trajectory(&sink);
	assert!(
		matches!(traj.outcome, TrajectoryOutcome::Error { .. }),
		"unreachable upstream must finalise as Error outcome; got {:?}",
		traj.outcome,
	);
}

// 4. l4_forward_factory_rejects_missing_upstream_arg

#[test]
fn l4_forward_factory_rejects_missing_upstream_arg() {
	// spec/crates/core.md § _Compile pipeline_: `args.upstream` is mandatory.
	// The factory's contract (per the public docstring on
	// `vane_engine::fetch::l4_forward::factory`) is that a missing or
	// non-string `upstream` yields a `FactoryError` whose message
	// references `upstream`.
	let result = l4_forward::factory(&serde_json::json!({}));
	let Err(FactoryError(msg)) = result else {
		panic!("missing upstream must error; got Ok(_)");
	};
	assert!(
		msg.contains("upstream"),
		"FactoryError message must reference the offending field; got {msg:?}",
	);
}

// 5. l4_forward_factory_rejects_empty_upstream_arg

#[test]
fn l4_forward_factory_rejects_empty_upstream_arg() {
	// Empty-string upstream is symbolically present but operationally
	// useless — `tokio::net::TcpStream::connect("")` would fail with an
	// uninformative parse error at runtime. The factory rejects up
	// front so misconfiguration surfaces at link time.
	let result = l4_forward::factory(&serde_json::json!({ "upstream": "" }));
	let Err(FactoryError(msg)) = result else {
		panic!("empty upstream must error; got Ok(_)");
	};
	assert!(
		msg.contains("upstream") || msg.contains("empty"),
		"FactoryError message must explain the empty-upstream rejection; got {msg:?}",
	);
}

// 6. l4_forward_handles_concurrent_connections

#[tokio::test]
async fn l4_forward_handles_concurrent_connections() {
	// Confirm per-connection isolation: each accepted connection dials
	// its own upstream socket and runs an independent `copy_bidirectional`
	// task. Cross-talk between connections would surface as a payload
	// mismatch on at least one client.
	let upstream_addr = spawn_echo_upstream().await;
	let proxy_addr = pick_port().await;
	let graph = make_proxy_graph(proxy_addr, &upstream_addr.to_string());

	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink_dyn);
	tokio::time::sleep(Duration::from_millis(50)).await;

	let mut handles = Vec::with_capacity(5);
	for i in 0..5u32 {
		let handle = tokio::spawn(async move {
			let payload = format!("payload-{i}").into_bytes();
			let mut client = TcpStream::connect(proxy_addr).await.expect("connect proxy");
			client.write_all(&payload).await.expect("write payload");
			client.shutdown().await.expect("client write shutdown");
			let mut received = Vec::new();
			client.read_to_end(&mut received).await.expect("read echoed payload");
			(payload, received)
		});
		handles.push(handle);
	}

	for handle in handles {
		let (expected, received) = handle.await.expect("client task joined");
		assert_eq!(received, expected, "each connection must echo its own payload — no cross-talk");
	}

	tokio::time::sleep(Duration::from_millis(50)).await;
	set.shutdown(Duration::from_secs(5)).await;
}

// 7. l4_forward_close_reason_graceful_emits_byte_tunnel_terminate_outcome

#[tokio::test]
async fn l4_forward_close_reason_graceful_emits_byte_tunnel_terminate_outcome() {
	// spec/crates/engine.md § _Fetch_: `L4ForwardFetch` constructs a
	// `Tunnel` with `close_reason_tx: None` (the L4 forward path does
	// not observe `CloseReason::Graceful` directly). The observable
	// surface for "tunnel completed cleanly" is therefore the per-
	// request Trajectory event whose outcome is
	// `Terminated { terminator: ByteTunnel, .. }`.
	let upstream_addr = spawn_echo_upstream().await;
	let proxy_addr = pick_port().await;
	let graph = make_proxy_graph(proxy_addr, &upstream_addr.to_string());

	let verbosity = Arc::new(VerbosityState::new());
	let sink = Arc::new(RecordingSink::new());
	let sink_dyn: Arc<dyn FlowLogSink> = Arc::clone(&sink) as Arc<dyn FlowLogSink>;

	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink_dyn);
	tokio::time::sleep(Duration::from_millis(50)).await;

	let mut client = TcpStream::connect(proxy_addr).await.expect("connect proxy");
	client.write_all(b"ok").await.expect("write payload");
	client.shutdown().await.expect("client write shutdown");
	let mut received = Vec::new();
	client.read_to_end(&mut received).await.expect("read echoed payload");
	assert_eq!(received, b"ok", "echo upstream must round-trip the payload");

	tokio::time::sleep(Duration::from_millis(50)).await;
	set.shutdown(Duration::from_millis(500)).await;

	let traj = first_trajectory(&sink);
	match traj.outcome {
		TrajectoryOutcome::Terminated { terminator, .. } => {
			assert_eq!(
				terminator,
				TerminatorOutcomeKind::ByteTunnel,
				"L4 forward path must finalise via the ByteTunnel terminator",
			);
		}
		other @ TrajectoryOutcome::Error { .. } => {
			panic!("expected Terminated{{ ByteTunnel, .. }}; got {other:?}");
		}
	}
}
