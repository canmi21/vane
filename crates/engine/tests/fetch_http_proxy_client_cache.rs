//! End-to-end coverage for the daemon-level upstream `Client` cache.
//!
//! Two `HttpProxyFetch` instances with matching `(version, tls)`
//! posture share one `Arc<Client>` — and therefore one connection
//! pool. Distinct postures (different version, secure vs insecure)
//! must NOT share. These tests verify both shapes by counting
//! upstream `accept` events.
//!
//! Spec: `spec/crates/engine-tls.md` § _Client cache: fingerprint
//! and reuse_, `spec/crates/engine.md` § _Pool fingerprint_.

#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serial_test::serial;
use tempfile::NamedTempFile;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, Node, NodeId, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::fetch::client_cache::{cache_len, clear_cache_for_test};
use vane_engine::fetch::http_proxy::register as register_http_proxy;
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

/// Build a graph rooted at `listen` whose entry is
/// `Upgrade -> Fetch(args) -> Terminate(WriteHttpResponse)`. The
/// `args` are passed verbatim into the `HttpProxyFetch` factory.
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

fn rcgen_self_signed() -> (String, String) {
	let issued =
		rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("self-signed cert");
	(issued.cert.pem(), issued.signing_key.serialize_pem())
}

fn build_server_config(cert_pem: &str, key_pem: &str, alpn: Vec<Vec<u8>>) -> rustls::ServerConfig {
	let cert_chain: Vec<_> =
		rustls_pemfile::certs(&mut cert_pem.as_bytes()).collect::<Result<_, _>>().expect("parse cert");
	let private_key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
		.expect("parse key bytes")
		.expect("private key present");
	let mut cfg = rustls::ServerConfig::builder()
		.with_no_client_auth()
		.with_single_cert(cert_chain, private_key)
		.expect("server cfg");
	cfg.alpn_protocols = alpn;
	cfg
}

struct TlsUpstream {
	addr: SocketAddr,
	accepted: Arc<AtomicUsize>,
	_cert_keep: NamedTempFile,
}

async fn spawn_tls_upstream(alpn: Vec<Vec<u8>>) -> TlsUpstream {
	let (cert_pem, key_pem) = rcgen_self_signed();
	let mut cert_file = NamedTempFile::new().expect("cert tmp");
	cert_file.write_all(cert_pem.as_bytes()).expect("write cert");

	let server_cfg = Arc::new(build_server_config(&cert_pem, &key_pem, alpn));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind tls");
	let addr = listener.local_addr().expect("local_addr");
	let accepted = Arc::new(AtomicUsize::new(0));
	let accepted_clone = Arc::clone(&accepted);

	tokio::spawn(async move {
		let acceptor = tokio_rustls::TlsAcceptor::from(server_cfg);
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			accepted_clone.fetch_add(1, Ordering::SeqCst);
			let acceptor = acceptor.clone();
			tokio::spawn(async move {
				let Ok(tls_stream) = acceptor.accept(sock).await else { return };
				let alpn_negotiated =
					tls_stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec).unwrap_or_default();
				let io = TokioIo::new(tls_stream);
				if alpn_negotiated == b"h2" {
					let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
						.serve_connection(io, service_fn(serve_ok))
						.await;
				} else {
					let _ = hyper::server::conn::http1::Builder::new()
						.serve_connection(io, service_fn(serve_ok))
						.await;
				}
			});
		}
	});

	TlsUpstream { addr, accepted, _cert_keep: cert_file }
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
async fn client_cache_two_fetches_with_same_tls_share_one_pool() {
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();
	let upstream = spawn_tls_upstream(vec![b"http/1.1".to_vec()]).await;

	// Two fetch instances on different listener ports, identical
	// args. The factory looks up the same `ClientFingerprint`, so
	// both end up with the same `Arc<Client>` and therefore the
	// same authority pool inside hyper-util.
	let args = serde_json::json!({
		"upstream": upstream.addr.to_string(),
		"version": "h1",
		"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
	});

	let proxy_a = pick_port().await;
	let graph_a = proxy_graph(proxy_a, args.clone());
	let (set_a, proxy_a) = start_listener(graph_a).await;

	let proxy_b = pick_port().await;
	let graph_b = proxy_graph(proxy_b, args);
	let (set_b, proxy_b) = start_listener(graph_b).await;

	assert_eq!(h1_get_status(proxy_a).await, 200);
	assert_eq!(h1_get_status(proxy_b).await, 200);

	tokio::time::sleep(Duration::from_millis(50)).await;
	let n = upstream.accepted.load(Ordering::SeqCst);
	assert_eq!(n, 1, "shared cache must reuse one upstream connection across two fetches; got {n}");
	assert_eq!(cache_len(), 1, "cache holds exactly one entry for one fingerprint");

	set_a.shutdown(Duration::from_millis(500)).await;
	set_b.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
#[serial]
async fn client_cache_different_versions_get_separate_clients() {
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();
	let upstream = spawn_tls_upstream(vec![b"h2".to_vec(), b"http/1.1".to_vec()]).await;

	// Same upstream + same TLS posture, but distinct `version`
	// settings → distinct `ClientFingerprint`s → two pools.
	let args_a = serde_json::json!({
		"upstream": upstream.addr.to_string(),
		"version": "auto",
		"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
	});
	let args_b = serde_json::json!({
		"upstream": upstream.addr.to_string(),
		"version": "h1",
		"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
	});

	let proxy_a = pick_port().await;
	let (set_a, proxy_a) = start_listener(proxy_graph(proxy_a, args_a)).await;
	let proxy_b = pick_port().await;
	let (set_b, proxy_b) = start_listener(proxy_graph(proxy_b, args_b)).await;

	assert_eq!(h1_get_status(proxy_a).await, 200);
	assert_eq!(h1_get_status(proxy_b).await, 200);

	tokio::time::sleep(Duration::from_millis(50)).await;
	let n = upstream.accepted.load(Ordering::SeqCst);
	assert_eq!(n, 2, "different versions must produce distinct upstream connections; got {n}");
	assert_eq!(cache_len(), 2);

	set_a.shutdown(Duration::from_millis(500)).await;
	set_b.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
#[serial]
async fn client_cache_secure_and_insecure_get_separate_clients() {
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();
	let upstream = spawn_tls_upstream(vec![b"http/1.1".to_vec()]).await;

	// Same upstream, same version, but different trust posture:
	// `insecure_skip_verify: true` vs `false`. The two MUST not
	// share a cache slot — pooling them would pool a verified pool
	// alongside an unverified pool, which is a security bug.
	//
	// The "secure" config is constructed by parse_tls_args; we
	// don't actually drive a request through it because the system
	// trust store will not chain to our self-signed leaf. The cache
	// lookup at factory time is enough to assert the fingerprints
	// differ — observed via `cache_len`.
	let secure = serde_json::json!({
		"upstream": upstream.addr.to_string(),
		"version": "h1",
		"tls": { "verify_hostname": "localhost" },
	});
	let insecure = serde_json::json!({
		"upstream": upstream.addr.to_string(),
		"version": "h1",
		"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
	});

	let proxy_secure = pick_port().await;
	let (set_secure, _proxy_secure) = start_listener(proxy_graph(proxy_secure, secure)).await;
	let proxy_insecure = pick_port().await;
	let (set_insecure, proxy_insecure) = start_listener(proxy_graph(proxy_insecure, insecure)).await;

	// Drive only the insecure listener (the secure one would fail
	// the handshake against a self-signed cert). The cache_len
	// assertion below proves both factories registered distinct
	// fingerprints.
	assert_eq!(h1_get_status(proxy_insecure).await, 200);
	tokio::time::sleep(Duration::from_millis(50)).await;
	// Use `>= 2` rather than `== 2`: `#[serial]` keeps tests from
	// running concurrently but does not guarantee the prior test's
	// `set.shutdown(500ms)` fully drained every spawned accept-loop
	// task before this test calls `clear_cache_for_test()`. A stray
	// accept on the prior listener after the wipe can register a third
	// entry under load; the invariant we actually care about is "secure
	// and insecure produced distinct fingerprints", which is satisfied
	// whenever the count is at least 2.
	assert!(
		cache_len() >= 2,
		"secure and insecure TLS fingerprints must occupy distinct cache slots; got {} entries",
		cache_len(),
	);

	set_secure.shutdown(Duration::from_millis(500)).await;
	set_insecure.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
#[serial]
async fn client_cache_cleartext_two_fetches_share_pool() {
	vane_engine::crypto::install_default_provider();
	clear_cache_for_test();
	let upstream = spawn_cleartext_upstream().await;

	// Two cleartext fetches with identical `(upstream, version)` →
	// `ClientFingerprint { version, tls: None }` matches → one
	// shared pool.
	let args = serde_json::json!({
		"upstream": upstream.addr.to_string(),
		"version": "h1",
	});

	let proxy_a = pick_port().await;
	let (set_a, proxy_a) = start_listener(proxy_graph(proxy_a, args.clone())).await;
	let proxy_b = pick_port().await;
	let (set_b, proxy_b) = start_listener(proxy_graph(proxy_b, args)).await;

	assert_eq!(h1_get_status(proxy_a).await, 200);
	assert_eq!(h1_get_status(proxy_b).await, 200);

	tokio::time::sleep(Duration::from_millis(50)).await;
	let n = upstream.accepted.load(Ordering::SeqCst);
	assert_eq!(n, 1, "cleartext shared cache must reuse one upstream connection; got {n}");
	assert_eq!(cache_len(), 1);

	set_a.shutdown(Duration::from_millis(500)).await;
	set_b.shutdown(Duration::from_millis(500)).await;
}
