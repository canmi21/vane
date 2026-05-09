//! End-to-end coverage for the UDP arm of `L4ForwardFetch` plus the
//! `listener_udp` dispatch table + cold/hot path discipline.
//!
//! These tests bind a real `UdpSocket` listener through `ListenerSet`,
//! drive client datagrams via `tokio::net::UdpSocket`, and observe the
//! upstream side via a second real `UdpSocket`. The fetch factory is
//! wrapped with a counter so we can assert the cold-path `FlowGraph`
//! entry fires exactly once per session — repeat datagrams from the
//! same peer take the hot path through the dispatch table.
//!
//! Spec: `spec/crates/engine.md` § _`udp_dispatch`_,
//! § _`udp_dispatch`_, § _`udp_dispatch`_.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use tokio::net::UdpSocket;
use vane_core::{
	ConnContext, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, FlowLogEvent, FlowLogSink,
	L4Conn, L4Fetch, Node, NodeId, SymbolicFetchRef, SymbolicFlowGraph, Terminator, TerminatorId,
	Transport, Tunnel,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::fetch::l4_forward;
use vane_engine::flow_graph::{FetchInst, FlowGraph};
use vane_engine::verbosity::VerbosityState;

/// L4 fetch wrapper that bumps `counter` on every `fetch()` call,
/// then delegates to `inner`. Lets the tests assert how many cold-path
/// `FlowGraph` entries actually fired (vs how many factory invocations
/// — those run once at link time and don't track runtime entries).
struct CountingL4Fetch {
	inner: Arc<dyn L4Fetch>,
	counter: Arc<AtomicUsize>,
}

#[async_trait]
impl L4Fetch for CountingL4Fetch {
	async fn fetch(
		&self,
		l4: L4Conn,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx,
	) -> Result<Tunnel, Error> {
		self.counter.fetch_add(1, Ordering::SeqCst);
		self.inner.fetch(l4, conn, ctx).await
	}
}

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

async fn pick_udp_port() -> SocketAddr {
	let s = UdpSocket::bind("127.0.0.1:0").await.expect("bind ephemeral udp for port pick");
	let addr = s.local_addr().expect("local_addr");
	drop(s);
	addr
}

fn meta_with_udp(addr: SocketAddr) -> FlowGraphMeta {
	let mut listener_transports = BTreeMap::new();
	listener_transports.insert(addr, Transport::Udp);
	FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls: BTreeMap::new(),
		listener_kinds: BTreeMap::new(),
		listener_transports,
		annotations: Vec::new(),
	}
}

/// Build a 2-node UDP forward graph rooted at `listen` and register
/// the `L4Forward` factory wrapped with a cold-path counter so tests
/// can assert hot/cold path behavior.
fn make_udp_proxy_graph(
	listen: SocketAddr,
	upstream: &str,
	idle_timeout: &str,
	cold_path_count: &Arc<AtomicUsize>,
) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));
	let args = serde_json::json!({
		"upstream": upstream,
		"transport": "udp",
		"idle_timeout": idle_timeout,
	});
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
			args,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		terminators: vec![Terminator::ByteTunnel],
		entries,
		meta: meta_with_udp(listen),
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	let counter = Arc::clone(cold_path_count);
	fetch.register(FetchKind::L4Forward, move |args| {
		// Build the real L4Forward fetch via the production factory,
		// then wrap it in a counter so every fetch() call (= every
		// cold-path FlowGraph entry) increments `cold_path_count`.
		// Factories themselves run only once at link time, so a
		// counter inside the factory closure measures linkage, not
		// runtime entries.
		let inner = match l4_forward::factory(args)? {
			FetchInst::L4(arc) => arc,
			FetchInst::L7(_) => unreachable!("L4Forward factory always emits FetchInst::L4"),
		};
		Ok(FetchInst::L4(Arc::new(CountingL4Fetch { inner, counter: Arc::clone(&counter) })))
	});
	FlowGraph::link(sym, &mw, &fetch).expect("link udp forward graph")
}

async fn start_udp_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("entries").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(&Arc::new(ArcSwap::new(graph)), &verbosity, &sink);
	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

/// Spawn a UDP echo upstream that replies to every datagram with a
/// fixed prefix so the test can observe the response leg too. Returns
/// `(addr, recv_count)` — the counter increments on every received
/// datagram.
async fn spawn_udp_echo() -> (SocketAddr, Arc<AtomicUsize>) {
	let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind echo");
	let addr = socket.local_addr().expect("local_addr");
	let counter = Arc::new(AtomicUsize::new(0));
	let counter_for_task = Arc::clone(&counter);
	tokio::spawn(async move {
		let mut buf = vec![0u8; 65535];
		loop {
			match socket.recv_from(&mut buf).await {
				Ok((n, peer)) => {
					counter_for_task.fetch_add(1, Ordering::SeqCst);
					let mut reply = Vec::with_capacity(n + 5);
					reply.extend_from_slice(b"echo:");
					reply.extend_from_slice(&buf[..n]);
					let _ = socket.send_to(&reply, peer).await;
				}
				Err(_) => return,
			}
		}
	});
	(addr, counter)
}

#[tokio::test]
async fn udp_forward_first_packet_reaches_upstream() {
	// Cold-path entry binds the upstream socket, sends every datagram in
	// `first_packets` (length-1 in the immediate cold-path case here),
	// and spawns the forwarder task. The upstream's recv counter must
	// observe the datagram exactly once.
	let (upstream_addr, upstream_recv) = spawn_udp_echo().await;
	let cold_path = Arc::new(AtomicUsize::new(0));
	let proxy_addr = pick_udp_port().await;
	let graph = make_udp_proxy_graph(proxy_addr, &upstream_addr.to_string(), "30s", &cold_path);
	let (set, proxy_addr) = start_udp_listener(graph).await;

	let client = UdpSocket::bind("127.0.0.1:0").await.expect("client bind");
	client.send_to(b"hello", proxy_addr).await.expect("send hello");

	tokio::time::sleep(Duration::from_millis(150)).await;
	assert_eq!(upstream_recv.load(Ordering::SeqCst), 1, "upstream must see exactly one datagram");
	assert_eq!(cold_path.load(Ordering::SeqCst), 1, "factory must build exactly one fetch instance");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn udp_forward_response_path_back_to_client() {
	// Hot-path leg: upstream replies; the forwarder sends the reply
	// through the listener's physical socket back to the original peer.
	let (upstream_addr, _upstream_recv) = spawn_udp_echo().await;
	let cold_path = Arc::new(AtomicUsize::new(0));
	let proxy_addr = pick_udp_port().await;
	let graph = make_udp_proxy_graph(proxy_addr, &upstream_addr.to_string(), "30s", &cold_path);
	let (set, proxy_addr) = start_udp_listener(graph).await;

	let client = UdpSocket::bind("127.0.0.1:0").await.expect("client bind");
	client.send_to(b"world", proxy_addr).await.expect("send world");

	let mut buf = vec![0u8; 1024];
	let recv = tokio::time::timeout(Duration::from_millis(500), client.recv_from(&mut buf))
		.await
		.expect("client recv timed out")
		.expect("recv ok");
	let (n, from) = recv;
	assert_eq!(from, proxy_addr, "reply must come from the listener address");
	assert_eq!(&buf[..n], b"echo:world", "client must see upstream's echo prefix");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn udp_forward_session_reuses_table_for_repeat_packets() {
	// Five datagrams from the same client peer must produce ONE
	// cold-path fetch invocation; the subsequent four take the hot
	// path through the dispatch table. Without that hot/cold split we
	// would see five fetches and five upstream sockets, defeating the
	// point of session-keyed dispatch.
	let (upstream_addr, upstream_recv) = spawn_udp_echo().await;
	let cold_path = Arc::new(AtomicUsize::new(0));
	let proxy_addr = pick_udp_port().await;
	let graph = make_udp_proxy_graph(proxy_addr, &upstream_addr.to_string(), "30s", &cold_path);
	let (set, proxy_addr) = start_udp_listener(graph).await;

	let client = UdpSocket::bind("127.0.0.1:0").await.expect("client bind");
	for i in 0u8..5 {
		client.send_to(&[b'p', i + b'0'], proxy_addr).await.expect("send");
		// Small spacing so the listener's recv loop can register the
		// session before the next packet arrives.
		tokio::time::sleep(Duration::from_millis(20)).await;
	}

	tokio::time::sleep(Duration::from_millis(100)).await;
	assert_eq!(upstream_recv.load(Ordering::SeqCst), 5, "all five datagrams must reach upstream");
	assert_eq!(
		cold_path.load(Ordering::SeqCst),
		1,
		"only the first datagram should trigger cold-path FlowGraph entry",
	);

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn udp_forward_session_dedupe_two_clients() {
	// Two distinct client source ports → two distinct DispatchKey
	// peers → two cold-path entries, two upstream sockets.
	let (upstream_addr, upstream_recv) = spawn_udp_echo().await;
	let cold_path = Arc::new(AtomicUsize::new(0));
	let proxy_addr = pick_udp_port().await;
	let graph = make_udp_proxy_graph(proxy_addr, &upstream_addr.to_string(), "30s", &cold_path);
	let (set, proxy_addr) = start_udp_listener(graph).await;

	let client_a = UdpSocket::bind("127.0.0.1:0").await.expect("client a bind");
	let client_b = UdpSocket::bind("127.0.0.1:0").await.expect("client b bind");
	client_a.send_to(b"a", proxy_addr).await.expect("send a");
	client_b.send_to(b"b", proxy_addr).await.expect("send b");

	tokio::time::sleep(Duration::from_millis(150)).await;
	assert_eq!(upstream_recv.load(Ordering::SeqCst), 2, "upstream must see two distinct datagrams");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn udp_forward_idle_timeout_reclaims_session() {
	// idle_timeout = 200ms. Send one datagram, wait past the window,
	// then send a second. The second packet must trigger a fresh
	// cold-path entry — the first session has been reclaimed.
	let (upstream_addr, _upstream_recv) = spawn_udp_echo().await;
	let cold_path = Arc::new(AtomicUsize::new(0));
	let proxy_addr = pick_udp_port().await;
	let graph = make_udp_proxy_graph(proxy_addr, &upstream_addr.to_string(), "200ms", &cold_path);
	let (set, proxy_addr) = start_udp_listener(graph).await;

	let client = UdpSocket::bind("127.0.0.1:0").await.expect("client bind");
	client.send_to(b"first", proxy_addr).await.expect("send first");
	tokio::time::sleep(Duration::from_millis(80)).await;
	assert_eq!(cold_path.load(Ordering::SeqCst), 1);

	// Wait past the idle window so the forwarder reclaims the session.
	tokio::time::sleep(Duration::from_millis(400)).await;
	client.send_to(b"second", proxy_addr).await.expect("send second");
	tokio::time::sleep(Duration::from_millis(150)).await;
	assert_eq!(
		cold_path.load(Ordering::SeqCst),
		2,
		"second datagram after idle timeout must re-enter the cold path",
	);

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn udp_forward_cancellation_kills_sessions() {
	// listener.shutdown() fires force_cancel; the per-session
	// drive_byte_tunnel arm propagates into the forwarder's cancel
	// token and the spawned task unwinds within the drain budget.
	let (upstream_addr, _upstream_recv) = spawn_udp_echo().await;
	let cold_path = Arc::new(AtomicUsize::new(0));
	let proxy_addr = pick_udp_port().await;
	let graph = make_udp_proxy_graph(proxy_addr, &upstream_addr.to_string(), "30s", &cold_path);
	let (set, proxy_addr) = start_udp_listener(graph).await;

	let client = UdpSocket::bind("127.0.0.1:0").await.expect("client bind");
	for i in 0u8..3 {
		client.send_to(&[b's', i + b'0'], proxy_addr).await.expect("send");
		tokio::time::sleep(Duration::from_millis(20)).await;
	}

	// Drain timeout must include the small force_cancel grace; bound
	// the entire shutdown to a wall-clock budget so a hung forwarder
	// surfaces as a failed assertion rather than a hung test.
	let started = std::time::Instant::now();
	tokio::time::timeout(Duration::from_secs(2), set.shutdown(Duration::from_millis(200)))
		.await
		.expect("listener shutdown must complete within timeout");
	assert!(
		started.elapsed() < Duration::from_secs(2),
		"shutdown must wind down sessions promptly under cancellation",
	);
}
