//! End-to-end tests for the H3 upstream path.
//!
//! Exercises an H1 client → vane → H3 upstream bridge: vane's
//! `HttpProxyFetch` with `version: "h3"` dials the upstream over QUIC
//! through the [`vane_engine::fetch::quic_pool`] singleton, sends the
//! request via `h3`, and streams the response body back through the
//! L7 executor as `Body::Stream(Box::pin(H3Body::new(...)))`. The
//! test-side H3 upstream is `vane_testutil::h3::serve_h3` (h3-quinn
//! server-side); production code in `vane-engine` does not depend on
//! `h3-quinn` for the upstream side either, but the test client
//! impersonating an H3 origin server is allowed to.
//!
//! Spec anchors:
//!
//! * `spec/architecture/07-l7.md` § _Architecture: TCP / QUIC
//!   separation_ — `QuicPool` ownership.
//! * `spec/architecture/07-l7.md` § _Pool fingerprint_ — two fetches
//!   sharing the same `(addr, tls_hash)` fingerprint share one entry.
//! * `spec/architecture/07-l7.md` § _Upstream-H3 send path_ — request
//!   body marshalling via `send_data` / `finish`, response body
//!   surfaced as `Body::Stream(...)`.
//! * `spec/architecture/08-tls.md` § _Upstream-side TLS_ —
//!   `insecure_skip_verify` covers the test's self-signed cert path.

#![cfg(feature = "h3")]
#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper_util::rt::TokioIo;
use tempfile::NamedTempFile;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, Node, NodeId, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::fetch::http_proxy::register as register_http_proxy;
use vane_engine::fetch::quic_pool;
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

// ---------------------------------------------------------------------------
// FlowLogSink fixture: drops events. Tests assert wire-level outcomes.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Self-signed cert for the H3 upstream. Mirrors the listener-tls test
// fixture pattern. Returns owned tempfiles so paths stay valid for the
// upstream's lifetime.
// ---------------------------------------------------------------------------

struct CertFixture {
	_cert_file: NamedTempFile,
	_key_file: NamedTempFile,
	cert_pem: String,
	key_pem: String,
	sni: String,
}

fn make_cert(sni: &str) -> CertFixture {
	let issued = rcgen::generate_simple_self_signed(vec![sni.to_owned()]).expect("self-signed cert");
	let cert_pem = issued.cert.pem();
	let key_pem = issued.signing_key.serialize_pem();
	let mut cert_file = NamedTempFile::new().expect("cert tmp");
	cert_file.write_all(cert_pem.as_bytes()).expect("write cert pem");
	let mut key_file = NamedTempFile::new().expect("key tmp");
	key_file.write_all(key_pem.as_bytes()).expect("write key pem");
	CertFixture { _cert_file: cert_file, _key_file: key_file, cert_pem, key_pem, sni: sni.to_owned() }
}

// ---------------------------------------------------------------------------
// Symbolic-graph factory for the H1 → H3 bridge: vane listens H1
// cleartext, proxies through H3 to the test's h3-quinn upstream.
// `args.tls` is the upstream TLS posture (insecure_skip_verify +
// verify_hostname = cert SNI), not the listener-side TLS — the H1
// listener is plaintext.
// ---------------------------------------------------------------------------

fn h3_proxy_graph(listen: SocketAddr, upstream: &str, sni: &str, insecure: bool) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));
	let meta = FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: std::collections::BTreeMap::new(),
		listener_tls: std::collections::BTreeMap::new(),
		listener_kinds: std::collections::BTreeMap::new(),
		listener_transports: std::collections::BTreeMap::new(),
	};

	let tls_args = serde_json::json!({
		"insecure_skip_verify": insecure,
		"verify_hostname": sni,
	});
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
			args: serde_json::json!({
				"upstream": upstream,
				"version": "h3",
				"tls": tls_args,
			}),
			retry_buffer_required: false,
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	register_http_proxy(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link h3 upstream graph")
}

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("entries").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(graph)), verbosity, sink);
	tokio::time::sleep(Duration::from_millis(100)).await;
	(set, addr)
}

// ---------------------------------------------------------------------------
// H1 client helpers — the vane listener is plaintext H1 in these tests,
// so the client side mirrors `fetch_http_proxy.rs`'s H1 setup.
// ---------------------------------------------------------------------------

async fn h1_client_empty(
	addr: SocketAddr,
) -> hyper::client::conn::http1::SendRequest<Empty<Bytes>> {
	let stream = tokio::net::TcpStream::connect(addr).await.expect("client connect");
	let io = TokioIo::new(stream);
	let (sender, conn) =
		hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io).await.expect("h1 handshake");
	tokio::spawn(async move {
		let _ = conn.await;
	});
	sender
}

async fn h1_client_full(addr: SocketAddr) -> hyper::client::conn::http1::SendRequest<Full<Bytes>> {
	let stream = tokio::net::TcpStream::connect(addr).await.expect("client connect");
	let io = TokioIo::new(stream);
	let (sender, conn) =
		hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io).await.expect("h1 handshake");
	tokio::spawn(async move {
		let _ = conn.await;
	});
	sender
}

// ---------------------------------------------------------------------------
// Echo handler: returns the request body verbatim with status 200.
// Used as the H3 upstream's request handler in most tests.
// ---------------------------------------------------------------------------

fn echo_handler(
	_req: http::Request<()>,
	body: Vec<u8>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = (http::StatusCode, Vec<u8>)> + Send>> {
	Box::pin(async move { (http::StatusCode::OK, body) })
}

// ---------------------------------------------------------------------------
// 1. GET round-trip H1 → H3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_upstream_get_round_trips_h1_to_h3() {
	vane_engine::crypto::install_default_provider();
	quic_pool::clear_for_test();

	let cert = make_cert("localhost");
	let upstream =
		vane_testutil::h3::serve_h3(&cert.cert_pem, &cert.key_pem, |_req, _body| async move {
			(http::StatusCode::OK, b"hello from h3 upstream".to_vec())
		})
		.await
		.expect("spawn h3 upstream");

	let proxy_addr = pick_port().await;
	let graph = h3_proxy_graph(proxy_addr, &upstream.addr.to_string(), &cert.sni, true);
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET");
	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "h3 upstream GET must surface 200");
	let body = resp.into_body().collect().await.expect("collect").to_bytes();
	assert_eq!(body.as_ref(), b"hello from h3 upstream", "upstream body must round-trip");

	set.shutdown(Duration::from_millis(500)).await;
	upstream.shutdown().await;
	drop(cert);
}

// ---------------------------------------------------------------------------
// 2. POST round-trip H1 → H3 with a small body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_upstream_post_round_trips_h1_to_h3() {
	vane_engine::crypto::install_default_provider();
	quic_pool::clear_for_test();

	let cert = make_cert("localhost");
	let upstream = vane_testutil::h3::serve_h3(&cert.cert_pem, &cert.key_pem, echo_handler)
		.await
		.expect("spawn h3 upstream");

	let proxy_addr = pick_port().await;
	let graph = h3_proxy_graph(proxy_addr, &upstream.addr.to_string(), &cert.sni, true);
	let (set, proxy_addr) = start_listener(graph).await;

	let payload: Vec<u8> = (0..2 * 1024).map(|i| u8::try_from(i & 0xff).unwrap_or(0)).collect();
	let mut sender = h1_client_full(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("POST")
		.uri("/echo")
		.header("host", "test.local")
		.body(Full::<Bytes>::new(Bytes::from(payload.clone())))
		.expect("build POST");
	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "h3 upstream POST must surface 200");
	let body = resp.into_body().collect().await.expect("collect").to_bytes();
	assert_eq!(body.len(), payload.len(), "echo body length");
	assert_eq!(body.as_ref(), payload.as_slice(), "echo body bytes");

	set.shutdown(Duration::from_millis(500)).await;
	upstream.shutdown().await;
	drop(cert);
}

// ---------------------------------------------------------------------------
// 3. Pool sharing — two requests against the same fingerprint reuse
//    one quic_pool entry. Asserted via the upstream's accept counter
//    (a fresh dial bumps it; reuse leaves it alone).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_upstream_pool_reuses_connection_across_requests() {
	vane_engine::crypto::install_default_provider();
	quic_pool::clear_for_test();

	let cert = make_cert("localhost");
	let upstream =
		vane_testutil::h3::serve_h3(&cert.cert_pem, &cert.key_pem, |_req, _body| async move {
			(http::StatusCode::OK, b"ok".to_vec())
		})
		.await
		.expect("spawn h3 upstream");

	let proxy_addr = pick_port().await;
	let graph = h3_proxy_graph(proxy_addr, &upstream.addr.to_string(), &cert.sni, true);
	let (set, proxy_addr) = start_listener(graph).await;

	for _ in 0..3 {
		let mut sender = h1_client_empty(proxy_addr).await;
		let req = hyper::Request::builder()
			.method("GET")
			.uri("/")
			.header("host", "test.local")
			.body(Empty::<Bytes>::new())
			.expect("build GET");
		let resp = sender.send_request(req).await.expect("send_request");
		assert_eq!(resp.status().as_u16(), 200);
		let _ = resp.into_body().collect().await.expect("collect");
	}

	assert_eq!(
		upstream.accept_count(),
		1,
		"three sequential requests against the same fingerprint must share one h3 connection",
	);
	assert_eq!(
		quic_pool::cache_len(),
		1,
		"pool must hold exactly one entry for the single fingerprint",
	);

	set.shutdown(Duration::from_millis(500)).await;
	upstream.shutdown().await;
	drop(cert);
}

// ---------------------------------------------------------------------------
// 4. TLS verify=skip accepts an unknown self-signed cert.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_upstream_verify_skip_accepts_unknown_cert() {
	vane_engine::crypto::install_default_provider();
	quic_pool::clear_for_test();

	let cert = make_cert("localhost");
	let upstream =
		vane_testutil::h3::serve_h3(&cert.cert_pem, &cert.key_pem, |_req, _body| async move {
			(http::StatusCode::OK, b"ok".to_vec())
		})
		.await
		.expect("spawn h3 upstream");

	let proxy_addr = pick_port().await;
	// insecure: true → NoVerify cert verifier; the unknown self-signed
	// cert is accepted; the connection succeeds.
	let graph = h3_proxy_graph(proxy_addr, &upstream.addr.to_string(), &cert.sni, true);
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET");
	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "verify=skip must accept self-signed cert");

	set.shutdown(Duration::from_millis(500)).await;
	upstream.shutdown().await;
	drop(cert);
}

// ---------------------------------------------------------------------------
// 5. TLS verify=full rejects the unknown self-signed cert with a
//    handshake failure surfaced as 502 from the L7 driver's synth.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_upstream_verify_full_rejects_unknown_cert() {
	vane_engine::crypto::install_default_provider();
	quic_pool::clear_for_test();

	let cert = make_cert("localhost");
	let upstream =
		vane_testutil::h3::serve_h3(&cert.cert_pem, &cert.key_pem, |_req, _body| async move {
			(http::StatusCode::OK, b"ok".to_vec())
		})
		.await
		.expect("spawn h3 upstream");

	let proxy_addr = pick_port().await;
	// insecure: false → use system trust store; the test's self-signed
	// cert isn't there, so the handshake fails. The H1 driver
	// synthesises a 500 for any L7 fetch error.
	let graph = h3_proxy_graph(proxy_addr, &upstream.addr.to_string(), &cert.sni, false);
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET");
	let resp = sender.send_request(req).await.expect("send_request");
	let status = resp.status().as_u16();
	assert!(
		(500..600).contains(&status),
		"verify=full against unknown cert must surface an upstream-error 5xx, got {status}",
	);

	set.shutdown(Duration::from_millis(500)).await;
	upstream.shutdown().await;
	drop(cert);
}

// ---------------------------------------------------------------------------
// 6. Dial failure: vane points at a UDP address with nothing
//    listening. The fetch surfaces upstream-unreachable; the H1
//    driver synthesises a 5xx.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_upstream_dial_failure_surfaces_5xx() {
	vane_engine::crypto::install_default_provider();
	quic_pool::clear_for_test();

	// Ephemeral bind on UDP, take the addr, drop the socket — likely
	// nothing else binds in the same instant, so the dial against this
	// addr fails.
	let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind probe");
	let dead_addr = probe.local_addr().expect("local_addr");
	drop(probe);

	let cert = make_cert("localhost");
	let proxy_addr = pick_port().await;
	let graph = h3_proxy_graph(proxy_addr, &dead_addr.to_string(), &cert.sni, true);
	let (set, proxy_addr) = start_listener(graph).await;

	let mut sender = h1_client_empty(proxy_addr).await;
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "test.local")
		.body(Empty::<Bytes>::new())
		.expect("build GET");

	// `HttpProxyFetch`'s H3 path caps the dial at 5s (spec default).
	// Wait 15s — well above the connect timeout and quinn's
	// post-timeout teardown — so a regression in the timeout wiring
	// surfaces as a test failure rather than a CI hang.
	let resp = tokio::time::timeout(Duration::from_secs(15), sender.send_request(req))
		.await
		.expect("h1 send_request must not hang past 15s")
		.expect("h1 send_request");
	let status = resp.status().as_u16();
	assert!(
		(500..600).contains(&status),
		"dial failure must surface as 5xx from the H1 driver, got {status}",
	);
	assert!(
		quic_pool::cache_len() == 0,
		"failed dial must not populate the pool; cache_len={}",
		quic_pool::cache_len(),
	);

	set.shutdown(Duration::from_millis(500)).await;
	drop(cert);
}
