//! End-to-end test for the `sni_peek` middleware factory wiring.
//!
//! Builds a hand-rolled `SymbolicFlowGraph` whose entry chain is
//! `Middleware(sni_peek) -> Check(tls.sni == "match.example.com")
//! -> Upgrade -> Fetch -> Terminate(WriteHttpResponse)`, links it via
//! the real `MiddlewareFactories::register("sni_peek", ...)` path, and
//! drives a TLS client at it. The match branch must serve `200/match`
//! when the client offers SNI `match.example.com`; the miss branch
//! serves `404/miss` for any other SNI.
//!
//! The test ratifies two contracts simultaneously:
//!
//! * The factory accepts the `"sni_peek"` rule name and produces an
//!   `L4Peek` middleware — without this, `FlowGraph::link` would reject
//!   the rule before any traffic flows.
//! * The listener's peek prelude pre-populates `ConnContext.tls.sni`
//!   from the parsed `ClientHello` (lowercase, per the SNI invariant)
//!   so the predicate evaluating `tls.sni == ...` sees a value.
//!
//! Spec anchors:
//!
//! * `spec/architecture/06-l4.md` § _Protocol detection_.

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
use serde_json::{Value, json};
use tempfile::NamedTempFile;
use vane_core::{
	Body, ConnContext, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, FlowLogEvent, FlowLogSink,
	L7Fetch, L7FetchOutput, MiddlewareId, Node, NodeId, PredicateId, PredicateInst, Request,
	Response, SymbolicFetchRef, SymbolicFlowGraph, SymbolicMiddlewareRef, Terminator, TerminatorId,
	predicate::{CompiledOperator, CompiledValue, FieldPath},
};
use vane_core::{MiddlewareKind, rule};
use vane_engine::ListenerSet;
use vane_engine::factories::{FactoryError, FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FetchInst, FlowGraph};
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

struct TlsFiles {
	_cert_file: NamedTempFile,
	_key_file: NamedTempFile,
	tls_cfg: rule::TlsConfig,
}

fn rcgen_default_cert() -> TlsFiles {
	// One self-signed cert covering both SNI labels we exercise. The
	// client side disables hostname verification (`NoVerify`) so SAN
	// content is irrelevant — we only need a cert the server hands back
	// regardless of which SNI the client offered.
	let issued = rcgen::generate_simple_self_signed(vec![
		"match.example.com".to_owned(),
		"other.example.com".to_owned(),
	])
	.expect("self-signed cert");
	let cert_pem = issued.cert.pem();
	let key_pem = issued.signing_key.serialize_pem();
	let mut cert_file = NamedTempFile::new().expect("cert tmp");
	cert_file.write_all(cert_pem.as_bytes()).expect("write cert pem");
	let mut key_file = NamedTempFile::new().expect("key tmp");
	key_file.write_all(key_pem.as_bytes()).expect("write key pem");
	let tls_cfg = rule::TlsConfig {
		sni: None,
		cert_file: cert_file.path().to_path_buf(),
		key_file: key_file.path().to_path_buf(),
	};
	TlsFiles { _cert_file: cert_file, _key_file: key_file, tls_cfg }
}

struct TaggedFetch {
	status: u16,
	body: &'static [u8],
}

#[async_trait]
impl L7Fetch for TaggedFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let resp: Response = http::Response::builder()
			.status(self.status)
			.body(Body::Static(Bytes::from_static(self.body)))
			.expect("build response");
		Ok(L7FetchOutput::Response(resp))
	}
}

fn tagged_fetch_factory(args: &Value) -> Result<FetchInst, FactoryError> {
	let tag = args
		.get("tag")
		.and_then(Value::as_str)
		.ok_or_else(|| FactoryError("missing tag".to_string()))?;
	let f: Arc<dyn L7Fetch> = match tag {
		"match" => Arc::new(TaggedFetch { status: 200, body: b"match" }),
		"miss" => Arc::new(TaggedFetch { status: 404, body: b"miss" }),
		other => return Err(FactoryError(format!("unknown tag {other}"))),
	};
	Ok(FetchInst::L7(f))
}

/// Hand-rolled symbolic graph:
///
/// ```text
///  0: Middleware(sni_peek) -> 1
///  1: Check(tls.sni == "match.example.com") on_match=2 on_miss=3
///  2: Upgrade -> 4
///  3: Upgrade -> 5
///  4: Fetch(match) -> 6
///  5: Fetch(miss)  -> 6
///  6: Terminate(WriteHttpResponse)
/// ```
fn sni_peek_branching_graph(addr: SocketAddr, tls_cfg: rule::TlsConfig) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));

	let mut listener_tls = BTreeMap::new();
	listener_tls
		.insert(addr, rule::ListenerTlsSpec { default: Some(tls_cfg), sni_certs: BTreeMap::new() });

	let meta = FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls,
	};

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
		Node::Upgrade { next: NodeId::new(4) },
		Node::Upgrade { next: NodeId::new(5) },
		Node::Fetch {
			id: FetchId::new(0),
			next_response: Some(NodeId::new(6)),
			next_tunnel: None,
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Fetch {
			id: FetchId::new(1),
			next_response: Some(NodeId::new(6)),
			next_tunnel: None,
			collect_body_before: None,
			body_limit: 0,
		},
		Node::Terminate(TerminatorId::new(0)),
	];

	let middlewares = vec![SymbolicMiddlewareRef {
		name: Arc::from("sni_peek"),
		args: json!({}),
		kind: MiddlewareKind::L4Peek,
		stateless: true,
		needs_body: false,
		on_error: None,
	}];

	let predicates = vec![PredicateInst {
		path: FieldPath::TlsSni,
		op: CompiledOperator::Equals(CompiledValue::Str(Arc::from("match.example.com"))),
	}];

	let fetches = vec![
		SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: json!({ "tag": "match" }) },
		SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: json!({ "tag": "miss" }) },
	];

	let sym = Arc::new(SymbolicFlowGraph {
		nodes,
		predicates,
		middlewares,
		fetches,
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});

	let mut mw = MiddlewareFactories::new();
	vane_engine::middleware::sni_peek::register(&mut mw);
	let mut fetch = FetchFactories::new();
	fetch.register(FetchKind::HttpSynthesize, tagged_fetch_factory);
	FlowGraph::link(sym, &mw, &fetch).expect("link sni_peek graph")
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

fn no_verify_h1_client_config() -> rustls::ClientConfig {
	let mut cfg = rustls::ClientConfig::builder()
		.dangerous()
		.with_custom_certificate_verifier(Arc::new(NoVerify))
		.with_no_client_auth();
	cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
	cfg
}

async fn drive_one_request(addr: SocketAddr, sni: &str) -> (u16, Bytes) {
	let connector = tokio_rustls::TlsConnector::from(Arc::new(no_verify_h1_client_config()));
	let tcp = tokio::net::TcpStream::connect(addr).await.expect("client tcp connect");
	let server_name = rustls::pki_types::ServerName::try_from(sni.to_owned()).expect("server name");
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
		.header("host", sni)
		.body(Empty::<Bytes>::new())
		.expect("build GET");
	let resp = sender.send_request(req).await.expect("send_request");
	let status = resp.status().as_u16();
	let body = resp.into_body().collect().await.expect("collect").to_bytes();
	(status, body)
}

#[tokio::test]
async fn sni_peek_routes_match_branch_when_sni_matches_predicate() {
	vane_engine::crypto::install_default_provider();
	let tls = rcgen_default_cert();
	let addr = pick_port().await;
	let graph = sni_peek_branching_graph(addr, tls.tls_cfg.clone());
	let (set, addr) = start_listener(graph).await;

	let (status, body) = drive_one_request(addr, "match.example.com").await;
	assert_eq!(status, 200, "match SNI must reach the match-tagged fetch");
	assert_eq!(body.as_ref(), b"match");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn sni_peek_routes_miss_branch_when_sni_differs() {
	vane_engine::crypto::install_default_provider();
	let tls = rcgen_default_cert();
	let addr = pick_port().await;
	let graph = sni_peek_branching_graph(addr, tls.tls_cfg.clone());
	let (set, addr) = start_listener(graph).await;

	let (status, body) = drive_one_request(addr, "other.example.com").await;
	assert_eq!(status, 404, "non-match SNI must reach the miss-tagged fetch");
	assert_eq!(body.as_ref(), b"miss");

	set.shutdown(Duration::from_millis(500)).await;
}
