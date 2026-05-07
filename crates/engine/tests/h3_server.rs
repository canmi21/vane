//! End-to-end tests for the H3 listener path.
//!
//! Exercises a real UDP H3 listener brought up against an HTTP/1.1 echo
//! upstream, driven by an h3 client (`vane_testutil::h3::connect_h3`)
//! that runs over `quinn::Endpoint::client` + `h3-quinn` + `h3::client`.
//! The test-side use of `h3-quinn` is fine — production code in
//! `vane-engine` does not depend on `h3-quinn`; only this test client
//! does.
//!
//! Spec anchors:
//!
//! * `spec/architecture/06-l4.md` § _UDP socket multiplexing: physical
//!   and virtual_ — the per-listener `quinn::Endpoint` model.
//! * `spec/architecture/06-l4.md` § _UDP listener semantics_ —
//!   `Http`-on-UDP listeners terminate H3 over QUIC.
//! * `spec/architecture/07-l7.md` § _`H3Body` (engine-owned)_ — the
//!   request-body streaming path that Step 3 wires up.

#![cfg(feature = "h3")]
#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tempfile::NamedTempFile;
use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, FlowLogEvent, FlowLogSink, ListenerKind, Node, NodeId,
	SymbolicFetchRef, SymbolicFlowGraph, Terminator, TerminatorId, Transport,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::fetch::http_proxy::register as register_http_proxy;
use vane_engine::flow_graph::FlowGraph;
use vane_engine::verbosity::VerbosityState;

// ---------------------------------------------------------------------------
// FlowLogSink fixture: drops events. These tests assert wire-level
// outcomes (response status, response body bytes); trajectory shape is
// covered elsewhere.
// ---------------------------------------------------------------------------

struct DropSink;

impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

// ---------------------------------------------------------------------------
// UDP free-port discovery — bind ephemeral on loopback v4, take
// `local_addr()`, drop. The `quinn::Endpoint` will rebind in the
// listener path. Race window between drop and listener bind is
// the same as the TCP free-port pattern used elsewhere.
// ---------------------------------------------------------------------------

async fn pick_udp_port() -> SocketAddr {
	let s = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind ephemeral udp");
	let addr = s.local_addr().expect("local_addr");
	drop(s);
	addr
}

// ---------------------------------------------------------------------------
// Self-signed cert fixture — same shape as `tests/listener_tls.rs`. The
// tempfiles are held by the fixture so the cert / key paths in
// `ListenerTlsSpec` stay valid for the listener's lifetime.
// ---------------------------------------------------------------------------

struct CertFixture {
	cert_file: NamedTempFile,
	key_file: NamedTempFile,
	cert_pem: String,
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
	CertFixture { cert_file, key_file, cert_pem, sni: sni.to_owned() }
}

// ---------------------------------------------------------------------------
// Symbolic-graph factory for the H3 path. Mirrors
// `tests/fetch_http_proxy.rs::proxy_graph` but populates the meta fields
// that the H3 listener path reads:
//
//   - listener_transports[addr] = Udp  → engine spawns run_udp_listener
//   - listener_kinds[addr]      = Http → run_udp_listener spawns h3 endpoint
//   - listener_tls[addr]        = ...  → cert resolver attached for TLS
// ---------------------------------------------------------------------------

fn h3_proxy_graph(listen: SocketAddr, upstream: &str, cert: &CertFixture) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));

	let mut listener_tls = BTreeMap::new();
	listener_tls.insert(
		listen,
		vane_core::rule::ListenerTlsSpec {
			default: Some(vane_core::rule::TlsConfig {
				sni: None,
				cert_file: Some(cert.cert_file.path().to_path_buf()),
				key_file: Some(cert.key_file.path().to_path_buf()),
				managed: None,
				client_auth: None,
				enable_zero_rtt: false,
			}),
			sni_certs: BTreeMap::new(),
			managed_snis: BTreeMap::new(),
			client_auth: vane_core::rule::ClientAuthSpec::None,
			enable_zero_rtt: false,
		},
	);

	let mut listener_kinds = BTreeMap::new();
	listener_kinds.insert(listen, ListenerKind::Http);

	let mut listener_transports = BTreeMap::new();
	listener_transports.insert(listen, Transport::Udp);

	let meta = FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls,
		listener_kinds,
		listener_transports,
	};

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
			args: serde_json::json!({ "upstream": upstream }),
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	register_http_proxy(&mut fetch, None);
	FlowGraph::link(sym, &mw, &fetch).expect("link h3 proxy graph")
}

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("entries").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(graph)), verbosity, sink);
	// Bind + h3 endpoint setup is async; give it a moment so the client
	// connect doesn't race the listener's UDP bind.
	tokio::time::sleep(Duration::from_millis(200)).await;
	(set, addr)
}

// ---------------------------------------------------------------------------
// Echo upstream — H1.1 server that returns the request body verbatim
// with status 200. Used as the proxy's upstream so we can assert end-to-
// end body round-trip.
// ---------------------------------------------------------------------------

async fn spawn_echo_upstream() -> SocketAddr {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind echo upstream");
	let addr = listener.local_addr().expect("upstream local_addr");
	tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			tokio::spawn(async move {
				let io = TokioIo::new(sock);
				let _ = hyper::server::conn::http1::Builder::new()
					.serve_connection(
						io,
						service_fn(|req: hyper::Request<hyper::body::Incoming>| async move {
							let body = req.into_body().collect().await.expect("collect").to_bytes();
							Ok::<_, Infallible>(
								hyper::Response::builder()
									.status(200)
									.body(Full::<Bytes>::new(body))
									.expect("build echo response"),
							)
						}),
					)
					.await;
			});
		}
	});
	addr
}

// ---------------------------------------------------------------------------
// Drain helper: walks `recv_data` until EOF, concatenating into one
// `Vec<u8>`. h3 yields `impl Buf` slices over quinn's internal buffer;
// the test consolidates them into a single contiguous vector for byte
// comparison.
// ---------------------------------------------------------------------------

async fn collect_h3_response_body(
	stream: &mut h3::client::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
) -> Vec<u8> {
	use bytes::Buf as _;
	let mut out = Vec::new();
	while let Some(mut chunk) = stream.recv_data().await.expect("h3 recv_data") {
		let remaining = chunk.remaining();
		out.extend_from_slice(&chunk.copy_to_bytes(remaining));
	}
	out
}

// ---------------------------------------------------------------------------
// 1. GET round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_get_round_trips_through_http_proxy() {
	vane_engine::crypto::install_default_provider();

	let upstream = spawn_echo_upstream().await;
	let cert = make_cert("localhost");
	let listen_addr = pick_udp_port().await;
	let graph = h3_proxy_graph(listen_addr, &upstream.to_string(), &cert);
	let (set, listen_addr) = start_listener(graph).await;

	let mut handle = vane_testutil::h3::connect_h3(listen_addr, &cert.cert_pem, &cert.sni)
		.await
		.expect("h3 client connect");

	let req = http::Request::builder()
		.method("GET")
		.uri("https://localhost/")
		.body(())
		.expect("build h3 GET");

	let mut stream = handle.send_request.send_request(req).await.expect("send_request");
	stream.finish().await.expect("finish request half");

	let resp = stream.recv_response().await.expect("recv_response");
	assert_eq!(resp.status().as_u16(), 200, "h3 GET must surface upstream 200");
	let body = collect_h3_response_body(&mut stream).await;
	assert!(body.is_empty(), "GET with no body must round-trip an empty response, got {body:?}");

	handle.shutdown().await;
	set.shutdown(Duration::from_millis(500)).await;
}

// ---------------------------------------------------------------------------
// 2. Small POST (1 KiB) round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_small_post_body_round_trips() {
	vane_engine::crypto::install_default_provider();

	let upstream = spawn_echo_upstream().await;
	let cert = make_cert("localhost");
	let listen_addr = pick_udp_port().await;
	let graph = h3_proxy_graph(listen_addr, &upstream.to_string(), &cert);
	let (set, listen_addr) = start_listener(graph).await;

	let mut handle = vane_testutil::h3::connect_h3(listen_addr, &cert.cert_pem, &cert.sni)
		.await
		.expect("h3 client connect");

	// 1 KiB of `b'a'` — small enough to fit in a single QUIC frame, large
	// enough to prove the body actually round-trips (vs an empty-body
	// no-op).
	let payload: Vec<u8> = vec![b'a'; 1024];

	let req = http::Request::builder()
		.method("POST")
		.uri("https://localhost/echo")
		.header("content-length", payload.len().to_string())
		.body(())
		.expect("build h3 POST");

	let mut stream = handle.send_request.send_request(req).await.expect("send_request");
	stream.send_data(Bytes::from(payload.clone())).await.expect("send_data");
	stream.finish().await.expect("finish request half");

	let resp = stream.recv_response().await.expect("recv_response");
	assert_eq!(resp.status().as_u16(), 200, "h3 POST must surface upstream 200");
	let body = collect_h3_response_body(&mut stream).await;
	assert_eq!(body, payload, "echo upstream must return the request body byte-for-byte");

	handle.shutdown().await;
	set.shutdown(Duration::from_millis(500)).await;
}

// ---------------------------------------------------------------------------
// 3. Larger POST (16 KiB) round-trip — regression net for body streaming
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_large_post_body_round_trips() {
	vane_engine::crypto::install_default_provider();

	let upstream = spawn_echo_upstream().await;
	let cert = make_cert("localhost");
	let listen_addr = pick_udp_port().await;
	let graph = h3_proxy_graph(listen_addr, &upstream.to_string(), &cert);
	let (set, listen_addr) = start_listener(graph).await;

	let mut handle = vane_testutil::h3::connect_h3(listen_addr, &cert.cert_pem, &cert.sni)
		.await
		.expect("h3 client connect");

	// 16 KiB — large enough to cross at least one QUIC frame boundary
	// at typical MTU, exercising the request-body assembly path the
	// H3Body streaming work is concerned with. Use a non-uniform
	// pattern so any byte-shift / off-by-one corruption surfaces in
	// the byte-equality assertion.
	let payload: Vec<u8> = (0..16u32 * 1024).map(|i| u8::try_from(i & 0xff).unwrap_or(0)).collect();

	let req = http::Request::builder()
		.method("POST")
		.uri("https://localhost/echo")
		.header("content-length", payload.len().to_string())
		.body(())
		.expect("build h3 POST");

	let mut stream = handle.send_request.send_request(req).await.expect("send_request");
	// Send in two chunks so the executor sees a multi-frame body.
	let (left, right) = payload.split_at(payload.len() / 2);
	stream.send_data(Bytes::copy_from_slice(left)).await.expect("send_data left");
	stream.send_data(Bytes::copy_from_slice(right)).await.expect("send_data right");
	stream.finish().await.expect("finish request half");

	let resp = stream.recv_response().await.expect("recv_response");
	assert_eq!(resp.status().as_u16(), 200, "h3 large POST must surface upstream 200");
	let body = collect_h3_response_body(&mut stream).await;
	assert_eq!(
		body.len(),
		payload.len(),
		"echo body length mismatch: expected {}, got {}",
		payload.len(),
		body.len(),
	);
	assert_eq!(body, payload, "echo upstream must return the 16 KiB body byte-for-byte");

	handle.shutdown().await;
	set.shutdown(Duration::from_millis(500)).await;
}
