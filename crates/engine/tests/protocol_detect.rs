//! End-to-end integration tests for the listener-side protocol-detect
//! prelude. Spawns a real TCP listener whose `FlowGraph` references a
//! capturing `L4Peek` middleware so the test observes what the peek
//! phase decided about each connection. Covers the four detector
//! outcomes called out in `spec/crates/engine.md` § _Protocol
//! detection_: HTTP/1, HTTP/2 preface, TLS `ClientHello` (with SNI
//! readable pre-handshake), and Unknown.
//!
//! The graph shape is `[Middleware(L4Peek) → Terminate(Close)]` —
//! capturing middleware records the parsed result and the connection
//! is dropped immediately afterwards. The point of these tests is the
//! detector + listener integration; downstream routing is covered by
//! the existing `fetch_*` / `executor` tests.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use vane_core::{
	ConnContext, Decision, DetectedProtocol, Error, FlowCtx, FlowGraphMeta, FlowLogEvent,
	FlowLogSink, L4PeekMiddleware, MiddlewareId, MiddlewareKind, Node, NodeId, PeekResult,
	SymbolicFetchRef, SymbolicFlowGraph, SymbolicMiddlewareRef, Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FlowGraph, MiddlewareInst};
use vane_engine::verbosity::VerbosityState;

#[derive(Clone, Debug)]
struct Captured {
	peek_len: usize,
	detected: Option<DetectedProtocol>,
	sni_in_conn_tls: Option<String>,
	tls_hello_sni: Option<String>,
}

struct CapturingPeek {
	captured: Arc<Mutex<Vec<Captured>>>,
}

#[async_trait]
impl L4PeekMiddleware for CapturingPeek {
	async fn run(
		&self,
		peek: &[u8],
		conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let user = conn.user.lock();
		let result = user.get::<PeekResult>();
		let detected = result.and_then(|r| r.detected);
		let tls_hello_sni = result.and_then(|r| r.tls.as_ref().and_then(|t| t.sni.clone()));
		drop(user);
		let sni_in_conn_tls = conn.tls.lock().as_ref().and_then(|t| t.sni.clone());
		self.captured.lock().push(Captured {
			peek_len: peek.len(),
			detected,
			sni_in_conn_tls,
			tls_hello_sni,
		});
		Ok(Decision::Continue)
	}
}

struct NullSink;
impl FlowLogSink for NullSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

async fn pick_port() -> SocketAddr {
	let l = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral");
	let addr = l.local_addr().expect("local_addr");
	drop(l);
	addr
}

fn meta() -> FlowGraphMeta {
	FlowGraphMeta {
		version_hash: [0u8; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: Vec::new(),
		feature_set: &[],
		short_circuit_response_entry: std::collections::BTreeMap::new(),
		listener_tls: std::collections::BTreeMap::new(),
		listener_kinds: std::collections::BTreeMap::new(),

		listener_transports: std::collections::BTreeMap::new(),
		annotations: Vec::new(),
	}
}

/// Build a graph rooted at `listen` with a single `L4Peek` middleware
/// pointing at a `Terminate(Close)`. The factory writes the captured
/// shared `Mutex` so each connection feeds the test.
fn build_graph(listen: SocketAddr, captured: &Arc<Mutex<Vec<Captured>>>) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(listen, NodeId::new(0));
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(1),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: Vec::new(),
		middlewares: vec![SymbolicMiddlewareRef {
			name: Arc::from("capturing_peek"),
			args: serde_json::json!({}),
			kind: MiddlewareKind::L4Peek,
			stateless: true,
			needs_body: false,
			on_error: None,
		}],
		fetches: Vec::<SymbolicFetchRef>::new(),
		terminators: vec![Terminator::Close],
		entries,
		meta: meta(),
	});
	let mut mw = MiddlewareFactories::new();
	let captured_for_factory = Arc::clone(captured);
	mw.register("capturing_peek", MiddlewareKind::L4Peek, move |_args| {
		Ok(MiddlewareInst::L4Peek(Arc::new(CapturingPeek {
			captured: Arc::clone(&captured_for_factory),
		}) as Arc<dyn L4PeekMiddleware>))
	});
	let fetch = FetchFactories::new();
	FlowGraph::link(sym, &mw, &fetch).expect("link peek graph")
}

/// Drive a single TCP connection that writes `payload` then half-
/// closes. Waits a short while afterwards so the listener's per-conn
/// task records its capture before the test drops the listener set.
async fn drive_client(addr: SocketAddr, payload: &[u8]) {
	let mut s = TcpStream::connect(addr).await.expect("client connect");
	s.write_all(payload).await.expect("client write");
	let _ = s.shutdown().await;
}

async fn await_capture(
	captured: &Arc<Mutex<Vec<Captured>>>,
	count: usize,
	deadline: Duration,
) -> Vec<Captured> {
	let start = std::time::Instant::now();
	while start.elapsed() < deadline {
		let snap = captured.lock().clone();
		if snap.len() >= count {
			return snap;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	captured.lock().clone()
}

const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

#[tokio::test]
async fn protocol_detect_classifies_http1_request() {
	let addr = pick_port().await;
	let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
	let graph = build_graph(addr, &captured);
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(NullSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink);
	tokio::time::sleep(Duration::from_millis(50)).await;

	drive_client(addr, b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await;

	let snap = await_capture(&captured, 1, Duration::from_secs(2)).await;
	let event = snap.first().expect("capture must arrive");
	assert_eq!(event.detected, Some(DetectedProtocol::Http1));
	assert!(event.peek_len > 0, "peek buffer must be non-empty for an Http1 request line");
	assert!(event.sni_in_conn_tls.is_none(), "non-TLS request must not populate ConnContext.tls.sni");
}

#[tokio::test]
async fn protocol_detect_classifies_http2_preface() {
	let addr = pick_port().await;
	let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
	let graph = build_graph(addr, &captured);
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(NullSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink);
	tokio::time::sleep(Duration::from_millis(50)).await;

	drive_client(addr, H2_PREFACE).await;

	let snap = await_capture(&captured, 1, Duration::from_secs(2)).await;
	let event = snap.first().expect("capture must arrive");
	assert_eq!(event.detected, Some(DetectedProtocol::Http2Preface));
	assert_eq!(event.peek_len, H2_PREFACE.len(), "peek buffer must contain exactly the preface");
}

#[tokio::test]
async fn protocol_detect_classifies_tls_clienthello_and_populates_sni() {
	let addr = pick_port().await;
	let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
	let graph = build_graph(addr, &captured);
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(NullSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink);
	tokio::time::sleep(Duration::from_millis(50)).await;

	let hello_bytes = build_client_hello_bytes("api.example.com", &[b"h2".to_vec()]);
	drive_client(addr, &hello_bytes).await;

	let snap = await_capture(&captured, 1, Duration::from_secs(2)).await;
	let event = snap.first().expect("capture must arrive");
	assert_eq!(event.detected, Some(DetectedProtocol::TlsClientHello));
	assert_eq!(
		event.tls_hello_sni.as_deref(),
		Some("api.example.com"),
		"PeekResult.tls.sni must surface the lowercased ClientHello name",
	);
	assert_eq!(
		event.sni_in_conn_tls.as_deref(),
		Some("api.example.com"),
		"ConnContext.tls.sni must be populated *before* any handshake (spec/crates/engine-tls.md § _SNI peek (L4)_)",
	);
}

#[tokio::test]
async fn protocol_detect_unknown_payload_routes_to_l4_subgraph() {
	let addr = pick_port().await;
	let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
	let graph = build_graph(addr, &captured);
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(NullSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(Arc::clone(&graph))), Arc::clone(&verbosity), sink);
	tokio::time::sleep(Duration::from_millis(50)).await;

	// Random-ish prefix that none of the detectors will commit on:
	// non-0x16 first byte, non-method ASCII, non-preface.
	let unknown = b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a";
	drive_client(addr, unknown).await;

	let snap = await_capture(&captured, 1, Duration::from_secs(2)).await;
	let event = snap.first().expect("capture must arrive");
	assert_eq!(
		event.detected,
		Some(DetectedProtocol::Unknown),
		"non-textual prefix with no TLS / H2 / H1 anchor must classify as Unknown",
	);
	assert!(event.sni_in_conn_tls.is_none(), "Unknown prefix must not populate SNI");
}

/// Synthesise a TLS `ClientHello` via rustls's own client-side state
/// machine. Same trick as the unit-level fixture in
/// `protocol_detect.rs`; duplicated here because the production
/// module's helper is `cfg(test)`-gated to its own crate.
fn build_client_hello_bytes(server_name: &str, alpn: &[Vec<u8>]) -> Vec<u8> {
	#[derive(Debug)]
	struct NoVerify;
	impl rustls::client::danger::ServerCertVerifier for NoVerify {
		fn verify_server_cert(
			&self,
			_end_entity: &rustls::pki_types::CertificateDer<'_>,
			_intermediates: &[rustls::pki_types::CertificateDer<'_>],
			_server_name: &rustls::pki_types::ServerName<'_>,
			_ocsp_response: &[u8],
			_now: rustls::pki_types::UnixTime,
		) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
			Ok(rustls::client::danger::ServerCertVerified::assertion())
		}
		fn verify_tls12_signature(
			&self,
			_message: &[u8],
			_cert: &rustls::pki_types::CertificateDer<'_>,
			_dss: &rustls::DigitallySignedStruct,
		) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
			Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
		}
		fn verify_tls13_signature(
			&self,
			_message: &[u8],
			_cert: &rustls::pki_types::CertificateDer<'_>,
			_dss: &rustls::DigitallySignedStruct,
		) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
			Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
		}
		fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
			rustls::crypto::CryptoProvider::get_default()
				.expect("crypto provider")
				.signature_verification_algorithms
				.supported_schemes()
		}
	}

	vane_engine::crypto::install_default_provider();

	let mut config = rustls::ClientConfig::builder()
		.dangerous()
		.with_custom_certificate_verifier(Arc::new(NoVerify))
		.with_no_client_auth();
	config.alpn_protocols = alpn.to_vec();
	let server =
		rustls::pki_types::ServerName::try_from(server_name.to_owned()).expect("server name");
	let mut conn = rustls::ClientConnection::new(Arc::new(config), server).expect("client conn");
	let mut out = Vec::new();
	conn.write_tls(&mut out).expect("write_tls");
	out
}
