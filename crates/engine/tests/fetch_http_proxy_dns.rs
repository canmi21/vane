//! End-to-end coverage for the per-rule DNS override knob.
//!
//! Exercises the wiring between `args.dns`, the daemon-level
//! [`vane_engine::fetch::client_cache::ClientFingerprint`], and the
//! `hyper-util` connector that now uses `hickory-resolver` instead of
//! the default `GaiResolver`. Two fingerprints differing only in their
//! [`vane_engine::fetch::dns::DnsConfig`] must occupy distinct cache
//! slots; bare-IPv6 nameservers must be rejected at factory time.
//!
//! Spec: `spec/crates/engine.md` § _DNS_,
//! `spec/crates/core.md` § _Compile pipeline_ (`dns` row).

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::json;
use serial_test::serial;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, Node, NodeId, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FactoryError, FetchFactories, MiddlewareFactories};
use vane_engine::fetch::client_cache::{cache_len, clear_cache_for_test};
use vane_engine::fetch::http_proxy::{
	factory as http_proxy_factory, register as register_http_proxy,
};
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

async fn pick_port() -> SocketAddr {
	let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral");
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
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls: BTreeMap::new(),
		listener_kinds: BTreeMap::new(),

		listener_transports: BTreeMap::new(),
		annotations: Vec::new(),
	}
}

fn proxy_graph(listen: SocketAddr, args: serde_json::Value) -> Arc<FlowGraph> {
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
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef {
			kind: FetchKind::HttpProxy,
			args,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta: sample_meta(),
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	register_http_proxy(&mut fetch, None);
	FlowGraph::link(sym, &mw, &fetch).expect("link http_proxy graph")
}

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("entries").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(graph)), verbosity, sink);
	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

async fn h1_get_status(proxy_addr: SocketAddr) -> u16 {
	let stream = tokio::net::TcpStream::connect(proxy_addr).await.expect("client connect");
	let io = TokioIo::new(stream);
	let (mut sender, conn) =
		hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io).await.expect("h1 handshake");
	tokio::spawn(async move {
		let _ = conn.await;
	});
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "localhost")
		.body(Empty::<Bytes>::new())
		.expect("build GET");
	let resp = sender.send_request(req).await.expect("send_request");
	let status = resp.status().as_u16();
	let _ = resp.into_body().collect().await;
	status
}

async fn serve_ok(
	_req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
	Ok(
		hyper::Response::builder()
			.status(200)
			.body(Full::<Bytes>::new(Bytes::from_static(b"ok")))
			.expect("build resp"),
	)
}

struct CleartextUpstream {
	addr: SocketAddr,
	accepted: Arc<AtomicUsize>,
}

async fn spawn_cleartext_upstream() -> CleartextUpstream {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind cleartext");
	let addr = listener.local_addr().expect("local_addr");
	let accepted = Arc::new(AtomicUsize::new(0));
	let accepted_clone = Arc::clone(&accepted);
	tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			accepted_clone.fetch_add(1, Ordering::SeqCst);
			tokio::spawn(async move {
				let io = TokioIo::new(sock);
				let _ = hyper::server::conn::http1::Builder::new()
					.serve_connection(io, service_fn(serve_ok))
					.await;
			});
		}
	});
	CleartextUpstream { addr, accepted }
}

#[tokio::test]
#[serial]
async fn dns_system_default_resolves_localhost() {
	// Drives an actual upstream request through the hickory `System`
	// path. Upstream is named by the `localhost` hostname (not
	// `127.0.0.1`) so the connector skips its IP-literal short-circuit
	// and reaches the resolver. /etc/hosts loading is part of hickory's
	// `system-config` feature, so this resolves to 127.0.0.1 on every
	// CI shape we target.
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();
	let upstream = spawn_cleartext_upstream().await;

	let args = json!({
		"upstream": format!("localhost:{}", upstream.addr.port()),
		"version": "h1",
	});

	let proxy_addr = pick_port().await;
	let (set, proxy_addr) = start_listener(proxy_graph(proxy_addr, args)).await;

	assert_eq!(h1_get_status(proxy_addr).await, 200);
	tokio::time::sleep(Duration::from_millis(50)).await;
	let n = upstream.accepted.load(Ordering::SeqCst);
	assert_eq!(n, 1, "system resolver must reach the upstream once; got {n}");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
#[serial]
async fn dns_custom_nameservers_appear_in_fingerprint() {
	// Two fetches share `(version, tls, upstream)` but differ in dns:
	// system vs custom. The factory must allocate two distinct cache
	// slots so a future request never reuses a Client whose resolver
	// would query the wrong nameserver. We don't drive the custom one
	// — the cache_len assertion is what matters.
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();

	http_proxy_factory(
		&json!({
			"upstream": "127.0.0.1:9999",
			"version": "h1",
		}),
		None,
	)
	.expect("system dns factory");

	http_proxy_factory(
		&json!({
			"upstream": "127.0.0.1:9999",
			"version": "h1",
			"dns": { "nameservers": ["127.0.0.1:53533"] },
		}),
		None,
	)
	.expect("custom dns factory");

	assert_eq!(cache_len(), 2, "system and custom dns must occupy distinct cache slots");
}

#[tokio::test]
#[serial]
async fn dns_nameserver_order_is_significant() {
	// `["1.1.1.1", "8.8.8.8"]` and `["8.8.8.8", "1.1.1.1"]` must
	// produce distinct fingerprints — operators express
	// primary/secondary intent through ordering. Sorting the list
	// behind the scenes would silently collapse the two into one
	// pool, defeating that intent.
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();

	http_proxy_factory(
		&json!({
			"upstream": "127.0.0.1:9999",
			"version": "h1",
			"dns": { "nameservers": ["1.1.1.1", "8.8.8.8"] },
		}),
		None,
	)
	.expect("primary 1.1.1.1");

	http_proxy_factory(
		&json!({
			"upstream": "127.0.0.1:9999",
			"version": "h1",
			"dns": { "nameservers": ["8.8.8.8", "1.1.1.1"] },
		}),
		None,
	)
	.expect("primary 8.8.8.8");

	assert_eq!(cache_len(), 2, "nameserver order must be load-bearing for the cache key");
}

#[tokio::test]
#[serial]
async fn dns_factory_rejects_bare_ipv6() {
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();
	let Err(FactoryError(msg)) = http_proxy_factory(
		&json!({
			"upstream": "127.0.0.1:9999",
			"version": "h1",
			"dns": { "nameservers": ["::1"] },
		}),
		None,
	) else {
		panic!("bare IPv6 must be rejected");
	};
	assert!(
		msg.contains("[IPv6]:port"),
		"error must steer the operator toward bracketed form: {msg}"
	);
}

#[tokio::test]
#[serial]
async fn dns_factory_accepts_bracketed_ipv6() {
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();
	http_proxy_factory(
		&json!({
			"upstream": "127.0.0.1:9999",
			"version": "h1",
			"dns": { "nameservers": ["[::1]:53"] },
		}),
		None,
	)
	.expect("bracketed IPv6 nameserver must be accepted");
}
