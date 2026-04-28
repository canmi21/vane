//! End-to-end tests for `ListenerKind` dispatch (Raw / Http / Auto).
//!
//! Builds hand-rolled `SymbolicFlowGraph` shapes that the engine's
//! link-time derivation classifies into each kind, runs them through
//! `ListenerSet::start`, and drives a real client at each variant of
//! the spec dispatch table:
//!
//! * Auto + TLS `ClientHello` + cert → `run_tls` then L7.
//! * Auto + cleartext H1 → cleartext H1 driver via `Node::Upgrade`.
//! * Auto + cleartext H2 preface → cleartext H2c driver via
//!   `Node::Upgrade` (the executor picks H2 from `conn.http_version`).
//! * Auto + TLS `ClientHello` + no cert → L4 subgraph (SNI passthrough).
//! * Http + cleartext → reject (connection closed without app bytes).
//! * Raw + any prefix → L4 subgraph (byte passthrough).
//!
//! Spec anchor: `spec/architecture/06-l4.md` § _Listener kind
//! derivation_ + § _Dispatch decision table_.

#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use vane_core::{
	Body, ConnContext, Decision, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, FlowLogEvent,
	FlowLogSink, L4PeekMiddleware, L7Fetch, L7FetchOutput, MiddlewareId, MiddlewareKind, Node,
	NodeId, PredicateId, PredicateInst, Request, Response, SymbolicFetchRef, SymbolicFlowGraph,
	SymbolicMiddlewareRef, Terminator, TerminatorId,
	predicate::{CompiledOperator, CompiledValue, FieldPath},
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::fetch::l4_forward;
use vane_engine::flow_graph::{FetchInst, FlowGraph, MiddlewareInst};
use vane_engine::verbosity::VerbosityState;

struct DropSink;
impl FlowLogSink for DropSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

async fn pick_port() -> SocketAddr {
	let l = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral");
	let addr = l.local_addr().expect("local_addr");
	drop(l);
	addr
}

fn sample_meta(
	listener_tls: BTreeMap<SocketAddr, vane_core::rule::ListenerTlsSpec>,
) -> FlowGraphMeta {
	FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls,
		listener_kinds: BTreeMap::new(),

		listener_transports: BTreeMap::new(),
	}
}

struct CertFiles {
	_cert: NamedTempFile,
	_key: NamedTempFile,
	cert_pem: String,
	tls_cfg: vane_core::rule::TlsConfig,
}

fn rcgen_cert(host: &str) -> CertFiles {
	let issued = rcgen::generate_simple_self_signed(vec![host.to_owned()]).expect("self-signed cert");
	let cert_pem = issued.cert.pem();
	let key_pem = issued.signing_key.serialize_pem();
	let mut cert = NamedTempFile::new().expect("cert tmp");
	cert.write_all(cert_pem.as_bytes()).expect("write cert");
	let mut key = NamedTempFile::new().expect("key tmp");
	key.write_all(key_pem.as_bytes()).expect("write key");
	let tls_cfg = vane_core::rule::TlsConfig {
		sni: None,
		cert_file: cert.path().to_path_buf(),
		key_file: key.path().to_path_buf(),
	};
	CertFiles { _cert: cert, _key: key, cert_pem, tls_cfg }
}

struct StaticOkFetch;

#[async_trait]
impl L7Fetch for StaticOkFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let resp: Response = http::Response::builder()
			.status(200)
			.body(Body::Static(Bytes::from_static(b"ok")))
			.expect("build response");
		Ok(L7FetchOutput::Response(resp))
	}
}

/// Marker `L4Peek` middleware that satisfies `flow_graph::needs_peek`
/// and otherwise lets the executor walk past untouched. Real
/// production rules use `vane_engine::middleware::sni_peek`; tests
/// register a local clone so they don't depend on the engine's
/// builtin registry.
struct PeekMarker;

#[async_trait]
impl L4PeekMiddleware for PeekMarker {
	async fn run(
		&self,
		_peek: &[u8],
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		Ok(Decision::Continue)
	}
}

fn register_peek_marker(mw: &mut MiddlewareFactories) {
	mw.register("peek_marker", MiddlewareKind::L4Peek, |_args| {
		Ok(MiddlewareInst::L4Peek(Arc::new(PeekMarker)))
	});
}

fn register_synth_ok(fetch: &mut FetchFactories) {
	fetch.register(FetchKind::HttpSynthesize, |_args: &Value| {
		Ok(FetchInst::L7(Arc::new(StaticOkFetch)))
	});
}

/// Auto graph: `Middleware(peek_marker) -> Check(tls.sni == "raw.example.com")
/// match=>L4Forward => ByteTunnel, miss=>Upgrade => Synthesize 200 ok =>
/// WriteHttpResponse`. Both L4 and L7 fetches reachable; lower derives Auto.
fn auto_graph(
	addr: SocketAddr,
	tls_cfg: Option<vane_core::rule::TlsConfig>,
	upstream: SocketAddr,
) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));

	let mut listener_tls = BTreeMap::new();
	if let Some(cfg) = tls_cfg {
		listener_tls.insert(
			addr,
			vane_core::rule::ListenerTlsSpec { default: Some(cfg), sni_certs: BTreeMap::new() },
		);
	}

	let nodes = vec![
		Node::Middleware {
			id: MiddlewareId::new(0),
			next: NodeId::new(1),
			on_error: None,
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Check {
			predicate: PredicateId::new(0),
			on_match: NodeId::new(2),
			on_miss: NodeId::new(3),
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Fetch {
			id: FetchId::new(0),
			next_response: None,
			next_tunnel: Some(NodeId::new(4)),
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Upgrade { next: NodeId::new(5) },
		Node::Terminate(TerminatorId::new(0)),
		Node::Fetch {
			id: FetchId::new(1),
			next_response: Some(NodeId::new(6)),
			next_tunnel: None,
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Terminate(TerminatorId::new(1)),
	];

	let middlewares = vec![SymbolicMiddlewareRef {
		name: Arc::from("peek_marker"),
		args: serde_json::json!({}),
		kind: MiddlewareKind::L4Peek,
		stateless: true,
		needs_body: false,
		on_error: None,
	}];

	let predicates = vec![PredicateInst {
		path: FieldPath::TlsSni,
		op: CompiledOperator::Equals(CompiledValue::Str(Arc::from("raw.example.com"))),
	}];

	let upstream_arg = serde_json::json!({ "upstream": upstream.to_string() });
	let fetches = vec![
		SymbolicFetchRef {
			kind: FetchKind::L4Forward,
			args: upstream_arg,
			retry_buffer_required: false,
		},
		SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: serde_json::Value::Null,
			retry_buffer_required: false,
		},
	];

	let sym = Arc::new(SymbolicFlowGraph {
		nodes,
		predicates,
		middlewares,
		fetches,
		terminators: vec![Terminator::ByteTunnel, Terminator::WriteHttpResponse],
		entries,
		meta: sample_meta(listener_tls),
	});

	let mut mw = MiddlewareFactories::new();
	register_peek_marker(&mut mw);
	let mut fetch = FetchFactories::new();
	l4_forward::register(&mut fetch);
	register_synth_ok(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link auto graph")
}

/// `Http` graph: `peek_marker` + Upgrade + Synthesize. Lower derives
/// `Http` (only L7 fetches reachable). With cert installed, TLS H1
/// works; a cleartext request hits the dispatch-table reject arm.
fn http_graph(addr: SocketAddr, tls_cfg: vane_core::rule::TlsConfig) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));

	let mut listener_tls = BTreeMap::new();
	listener_tls.insert(
		addr,
		vane_core::rule::ListenerTlsSpec { default: Some(tls_cfg), sni_certs: BTreeMap::new() },
	);

	let nodes = vec![
		Node::Middleware {
			id: MiddlewareId::new(0),
			next: NodeId::new(1),
			on_error: None,
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Upgrade { next: NodeId::new(2) },
		Node::Fetch {
			id: FetchId::new(0),
			next_response: Some(NodeId::new(3)),
			next_tunnel: None,
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Terminate(TerminatorId::new(0)),
	];

	let middlewares = vec![SymbolicMiddlewareRef {
		name: Arc::from("peek_marker"),
		args: serde_json::json!({}),
		kind: MiddlewareKind::L4Peek,
		stateless: true,
		needs_body: false,
		on_error: None,
	}];

	let sym = Arc::new(SymbolicFlowGraph {
		nodes,
		predicates: vec![],
		middlewares,
		fetches: vec![SymbolicFetchRef {
			kind: FetchKind::HttpSynthesize,
			args: serde_json::Value::Null,
			retry_buffer_required: false,
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta: sample_meta(listener_tls),
	});

	let mut mw = MiddlewareFactories::new();
	register_peek_marker(&mut mw);
	let mut fetch = FetchFactories::new();
	register_synth_ok(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link http graph")
}

/// Raw graph: single `L4Forward` fetch. Lower derives Raw (no L7).
fn raw_graph(addr: SocketAddr, upstream: SocketAddr) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));

	let nodes = vec![
		Node::Fetch {
			id: FetchId::new(0),
			next_response: None,
			next_tunnel: Some(NodeId::new(1)),
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Terminate(TerminatorId::new(0)),
	];

	let upstream_arg = serde_json::json!({ "upstream": upstream.to_string() });
	let sym = Arc::new(SymbolicFlowGraph {
		nodes,
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef {
			kind: FetchKind::L4Forward,
			args: upstream_arg,
			retry_buffer_required: false,
		}],
		terminators: vec![Terminator::ByteTunnel],
		entries,
		meta: sample_meta(BTreeMap::new()),
	});

	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	l4_forward::register(&mut fetch);
	FlowGraph::link(sym, &mw, &fetch).expect("link raw graph")
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

/// Spawn a TCP echo server on a free port. Each accepted connection
/// reads bytes until EOF and writes them back. Returns the socket
/// addr; the spawned task lives for the test's duration.
async fn spawn_echo() -> SocketAddr {
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind echo");
	let addr = listener.local_addr().expect("echo addr");
	tokio::spawn(async move {
		loop {
			let Ok((mut sock, _)) = listener.accept().await else { return };
			tokio::spawn(async move {
				let mut buf = vec![0u8; 4096];
				while let Ok(n) = sock.read(&mut buf).await {
					if n == 0 || sock.write_all(&buf[..n]).await.is_err() {
						break;
					}
				}
			});
		}
	});
	addr
}

#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
	fn verify_server_cert(
		&self,
		_end_entity: &rustls::pki_types::CertificateDer<'_>,
		_intermediates: &[rustls::pki_types::CertificateDer<'_>],
		_server_name: &rustls::pki_types::ServerName<'_>,
		_ocsp: &[u8],
		_now: rustls::pki_types::UnixTime,
	) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
		Ok(rustls::client::danger::ServerCertVerified::assertion())
	}
	fn verify_tls12_signature(
		&self,
		_msg: &[u8],
		_cert: &rustls::pki_types::CertificateDer<'_>,
		_dss: &rustls::DigitallySignedStruct,
	) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
		Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
	}
	fn verify_tls13_signature(
		&self,
		_msg: &[u8],
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

fn h1_client_config_trusting(server_cert_pem: &str) -> rustls::ClientConfig {
	let mut roots = rustls::RootCertStore::empty();
	for cert in rustls_pemfile::certs(&mut server_cert_pem.as_bytes()) {
		roots.add(cert.expect("parse cert")).expect("add cert");
	}
	let mut cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
	cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
	cfg
}

#[tokio::test]
async fn auto_listener_serves_tls_h1_when_client_sends_clienthello() {
	vane_engine::crypto::install_default_provider();

	let cert = rcgen_cert("localhost");
	let echo = spawn_echo().await;
	let addr = pick_port().await;
	let graph = auto_graph(addr, Some(cert.tls_cfg.clone()), echo);
	let (set, addr) = start_listener(graph).await;

	let connector =
		tokio_rustls::TlsConnector::from(Arc::new(h1_client_config_trusting(&cert.cert_pem)));
	let tcp = TcpStream::connect(addr).await.expect("tcp connect");
	let server_name = rustls::pki_types::ServerName::try_from("localhost").expect("server name");
	let tls_stream = connector.connect(server_name, tcp).await.expect("tls handshake");

	let io = TokioIo::new(tls_stream);
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
	let resp = sender.send_request(req).await.expect("send request");
	assert_eq!(resp.status().as_u16(), 200);
	let body = resp.into_body().collect().await.expect("collect").to_bytes();
	assert_eq!(body.as_ref(), b"ok");
	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn auto_listener_serves_cleartext_h1_when_client_sends_plain_get() {
	vane_engine::crypto::install_default_provider();

	let cert = rcgen_cert("localhost");
	let echo = spawn_echo().await;
	let addr = pick_port().await;
	let graph = auto_graph(addr, Some(cert.tls_cfg), echo);
	let (set, addr) = start_listener(graph).await;

	let mut s = TcpStream::connect(addr).await.expect("tcp connect");
	s.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
		.await
		.expect("write request");
	let mut buf = Vec::new();
	s.read_to_end(&mut buf).await.expect("read response");
	let response_text = String::from_utf8_lossy(&buf);
	assert!(response_text.starts_with("HTTP/1.1 200"), "{response_text}");
	assert!(response_text.ends_with("ok"), "body is 'ok': {response_text}");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn auto_listener_serves_cleartext_h2c_when_client_sends_h2_preface() {
	vane_engine::crypto::install_default_provider();

	let cert = rcgen_cert("localhost");
	let echo = spawn_echo().await;
	let addr = pick_port().await;
	let graph = auto_graph(addr, Some(cert.tls_cfg), echo);
	let (set, addr) = start_listener(graph).await;

	// hyper H2 client over a plain TCP socket. The server sees the
	// `PRI * HTTP/2.0…` preface before any other bytes; the listener's
	// peek prelude classifies it as `Http2Preface`, dispatches to the
	// cleartext h2c branch, and the executor's `Node::Upgrade` arm
	// drives `drive_h2_server` (picked because the listener pre-set
	// `conn.http_version = Http2`).
	let tcp = TcpStream::connect(addr).await.expect("tcp connect");
	let io = TokioIo::new(tcp);
	let (mut sender, conn) = hyper::client::conn::http2::handshake::<_, _, Empty<Bytes>>(
		hyper_util::rt::TokioExecutor::new(),
		io,
	)
	.await
	.expect("h2 handshake");
	tokio::spawn(async move {
		let _ = conn.await;
	});

	let req = hyper::Request::builder()
		.method("GET")
		.uri("http://localhost/")
		.body(Empty::<Bytes>::new())
		.expect("build GET");
	let resp = sender.send_request(req).await.expect("send request");
	assert_eq!(resp.status().as_u16(), 200);
	let body = resp.into_body().collect().await.expect("collect").to_bytes();
	assert_eq!(body.as_ref(), b"ok");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn auto_listener_passthrough_l4_when_tls_clienthello_but_no_cert() {
	vane_engine::crypto::install_default_provider();

	// Auto graph WITHOUT cert. Client sends a TLS ClientHello whose SNI
	// matches the L4 branch; the listener has no cert so it cannot
	// terminate, but the dispatch table sends the bytes unmodified
	// into the L4 subgraph (`L4Forward` → echo upstream). The echo
	// echoes the ClientHello bytes back, which the client reads as a
	// raw byte stream — no handshake completes.
	let echo = spawn_echo().await;
	let addr = pick_port().await;
	let graph = auto_graph(addr, None, echo);
	let (set, addr) = start_listener(graph).await;

	// Construct a minimal ClientHello prefix: TLS record header
	// (0x16 0x03 0x01 ..length..) + handshake header (0x01 client_hello,
	// 3 length bytes) + the bare minimum body the spec considers a
	// well-formed enough hello for the peek classifier. We don't need
	// full validity — the listener doesn't decrypt; it just looks at
	// the first byte and the ALPN/SNI extensions to populate
	// `PeekResult.tls`. For passthrough, even a partial ClientHello is
	// fine because the L4 branch echoes it as-is.
	let payload = build_client_hello_for("raw.example.com");
	let mut s = TcpStream::connect(addr).await.expect("tcp connect");
	s.write_all(&payload).await.expect("write hello bytes");
	let mut echoed = vec![0u8; payload.len()];
	let n = tokio::time::timeout(Duration::from_secs(2), s.read_exact(&mut echoed))
		.await
		.expect("echo within deadline")
		.expect("read echoed bytes");
	assert_eq!(n, payload.len());
	assert_eq!(&echoed, &payload, "L4 passthrough must echo the ClientHello bytes verbatim");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn http_listener_rejects_cleartext_get() {
	vane_engine::crypto::install_default_provider();

	let cert = rcgen_cert("localhost");
	let addr = pick_port().await;
	let graph = http_graph(addr, cert.tls_cfg);
	let (set, addr) = start_listener(graph).await;

	let mut s = TcpStream::connect(addr).await.expect("tcp connect");
	s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.expect("write");
	let mut buf = Vec::new();
	let _ = tokio::time::timeout(Duration::from_secs(2), s.read_to_end(&mut buf))
		.await
		.expect("server hangs up within deadline");
	let body = String::from_utf8_lossy(&buf);
	assert!(
		!body.contains("ok") && !body.starts_with("HTTP/1.1 200"),
		"Http listener must not serve the L7 response on cleartext: got {body:?}",
	);

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn raw_listener_passthrough_for_any_prefix() {
	vane_engine::crypto::install_default_provider();

	let echo = spawn_echo().await;
	let addr = pick_port().await;
	let graph = raw_graph(addr, echo);
	let (set, addr) = start_listener(graph).await;

	for prefix in
		[b"GET / HTTP/1.1\r\n".as_ref(), &[0x16u8, 0x03, 0x01, 0x00, 0x05, 0x01, 0x00, 0x00]]
	{
		let mut s = TcpStream::connect(addr).await.expect("tcp connect");
		s.write_all(prefix).await.expect("write");
		let mut echoed = vec![0u8; prefix.len()];
		let n = tokio::time::timeout(Duration::from_secs(2), s.read_exact(&mut echoed))
			.await
			.expect("echo within deadline")
			.expect("read echoed bytes");
		assert_eq!(n, prefix.len());
		assert_eq!(&echoed, prefix, "Raw listener must passthrough every prefix verbatim");
	}

	set.shutdown(Duration::from_millis(500)).await;
}

/// Build a minimal TLS 1.2 `ClientHello` bytes blob with the given SNI.
/// Just enough for the peek classifier to commit to `TlsClientHello`
/// and write `ctx.tls.sni`. Adapted from
/// `crates/engine/tests/protocol_detect.rs::build_client_hello_bytes`.
fn build_client_hello_for(sni: &str) -> Vec<u8> {
	use rustls::pki_types::ServerName;
	let server_name = ServerName::try_from(sni.to_owned()).expect("server name");
	let mut cfg = rustls::ClientConfig::builder()
		.dangerous()
		.with_custom_certificate_verifier(Arc::new(NoVerify))
		.with_no_client_auth();
	cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
	let cfg = Arc::new(cfg);
	let mut conn = rustls::ClientConnection::new(cfg, server_name).expect("client conn");
	let mut out = Vec::new();
	conn.write_tls(&mut out).expect("write_tls returns first flight");
	out
}
