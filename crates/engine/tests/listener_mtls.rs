//! End-to-end tests for listener-side mTLS.
//!
//! Builds a `SymbolicFlowGraph` whose `listener_tls.client_auth` is
//! either `Request` or `Require`, drives a real `tokio_rustls` client
//! at it with / without a CA-signed client cert, and asserts:
//!
//! * `Require` rejects clients without (or with a wrong-CA) cert at
//!   the handshake stage; valid-cert clients reach the upstream.
//! * `Request` lets cert-less clients through; rules using
//!   `tls.peer_cert.present` / `tls.peer_cert.subject_cn` route to
//!   distinct fetches based on what (if anything) the client
//!   presented.
//! * The seven `tls.peer_cert.*` predicate fields populate from the
//!   verified leaf in `ConnContext.tls.peer_cert`. Two are exercised
//!   end-to-end here (`subject_cn`, `san_dns`); the other five
//!   (`present`, `fingerprint_sha256`, `spki_sha256`, `issuer_cn`,
//!   `serial`) have unit-test coverage in vane-core.
//!
//! Spec: `spec/crates/engine-tls.md` § _Client certificate
//! verification (mTLS on listener)_ + § _CRL_.

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
use vane_core::{
	Body, ConnContext, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, FlowLogEvent, FlowLogSink,
	L7Fetch, L7FetchOutput, Node, NodeId, PredicateId, PredicateInst, Request, SymbolicFetchRef,
	SymbolicFlowGraph, Terminator, TerminatorId,
	predicate::{CompiledOperator, CompiledValue, FieldPath},
};
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

// rcgen helpers — issue a CA + server cert + client cert chain
struct PkiFixture {
	ca_pem: String,
	_ca_file: NamedTempFile,
	ca_path: std::path::PathBuf,
	#[allow(dead_code)]
	server_cert_pem: String,
	#[allow(dead_code)]
	server_cert_file: NamedTempFile,
	#[allow(dead_code)]
	server_key_file: NamedTempFile,
	server_tls_cfg: vane_core::rule::TlsConfig,
	client_cert_chain: Vec<rustls_pki_types::CertificateDer<'static>>,
	client_key: rustls_pki_types::PrivateKeyDer<'static>,
	#[allow(dead_code)]
	client_subject_cn: String,
	#[allow(dead_code)]
	client_san_dns: Vec<String>,
}

fn rcgen_pki(client_cn: &str, client_san: &[&str]) -> PkiFixture {
	// CA: self-signed root, then wrapped in `Issuer` for downstream signing.
	let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("ca params");
	ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
	ca_params.distinguished_name = rcgen::DistinguishedName::new();
	ca_params.distinguished_name.push(rcgen::DnType::CommonName, "vane-test-ca");
	let ca_key = rcgen::KeyPair::generate().expect("ca key");
	let ca = ca_params.self_signed(&ca_key).expect("self-sign ca");
	let ca_pem = ca.pem();
	let mut ca_file = NamedTempFile::new().expect("ca tmp");
	ca_file.write_all(ca_pem.as_bytes()).expect("write ca pem");
	let ca_path = ca_file.path().to_path_buf();
	let issuer: rcgen::Issuer<'_, rcgen::KeyPair> = rcgen::Issuer::new(ca_params, ca_key);

	// Server cert (CA-signed, SAN = localhost)
	let mut server_params =
		rcgen::CertificateParams::new(vec!["localhost".to_owned()]).expect("server params");
	server_params.distinguished_name = rcgen::DistinguishedName::new();
	server_params.distinguished_name.push(rcgen::DnType::CommonName, "vane-test-server");
	let server_key = rcgen::KeyPair::generate().expect("server key");
	let server_cert = server_params.signed_by(&server_key, &issuer).expect("ca-sign server");
	let server_cert_pem = server_cert.pem();
	let mut server_cert_file = NamedTempFile::new().expect("server cert tmp");
	server_cert_file.write_all(server_cert_pem.as_bytes()).expect("write server cert");
	let mut server_key_file = NamedTempFile::new().expect("server key tmp");
	server_key_file.write_all(server_key.serialize_pem().as_bytes()).expect("write server key");
	let server_tls_cfg = vane_core::rule::TlsConfig {
		sni: None,
		cert_file: Some(server_cert_file.path().to_path_buf()),
		key_file: Some(server_key_file.path().to_path_buf()),
		managed: None,
		client_auth: None, // populated per-test
		enable_zero_rtt: false,
		ocsp_path: None,
		ocsp_fetch: false,
	};

	// Client cert (CA-signed)
	let san_owned: Vec<String> = client_san.iter().map(|s| (*s).to_owned()).collect();
	let mut client_params = rcgen::CertificateParams::new(san_owned.clone()).expect("client params");
	client_params.distinguished_name = rcgen::DistinguishedName::new();
	client_params.distinguished_name.push(rcgen::DnType::CommonName, client_cn);
	let client_key_pair = rcgen::KeyPair::generate().expect("client key");
	let client_cert = client_params.signed_by(&client_key_pair, &issuer).expect("ca-sign client");
	let client_cert_chain = vec![client_cert.der().clone()];
	let client_key_pkcs8 = client_key_pair.serialize_der();
	let client_key = rustls_pki_types::PrivateKeyDer::Pkcs8(
		rustls_pki_types::PrivatePkcs8KeyDer::from(client_key_pkcs8),
	);

	PkiFixture {
		ca_pem,
		_ca_file: ca_file,
		ca_path,
		server_cert_pem,
		server_cert_file,
		server_key_file,
		server_tls_cfg,
		client_cert_chain,
		client_key,
		client_subject_cn: client_cn.to_owned(),
		client_san_dns: san_owned,
	}
}

// L7 fetch that tags responses with a fixed body
struct TaggedFetch {
	tag: &'static str,
}
#[async_trait]
impl L7Fetch for TaggedFetch {
	async fn fetch(
		&self,
		_req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		Ok(L7FetchOutput::Response(
			http::Response::builder()
				.status(200)
				.body(Body::Static(Bytes::from_static(b"ok")))
				.expect("build response")
				.map(|_| Body::Static(Bytes::copy_from_slice(self.tag.as_bytes()))),
		))
	}
}

fn tagged_fetch_factory(args: &Value) -> Result<FetchInst, FactoryError> {
	let tag = args
		.get("tag")
		.and_then(Value::as_str)
		.ok_or_else(|| FactoryError("missing tag".to_string()))?;
	let static_tag: &'static str = match tag {
		"with-cert" => "with-cert",
		"without-cert" => "without-cert",
		"valid" => "valid",
		other => return Err(FactoryError(format!("unknown tag {other}"))),
	};
	Ok(FetchInst::L7(Arc::new(TaggedFetch { tag: static_tag })))
}

// Graph builders
fn meta_with_client_auth(
	addr: SocketAddr,
	server: vane_core::rule::TlsConfig,
	client_auth: vane_core::rule::ClientAuthSpec,
) -> FlowGraphMeta {
	let mut listener_tls = BTreeMap::new();
	listener_tls.insert(
		addr,
		vane_core::rule::ListenerTlsSpec {
			default: Some(server),
			sni_certs: BTreeMap::new(),
			managed_snis: BTreeMap::new(),
			client_auth,
			enable_zero_rtt: false,
		},
	);
	FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls,
		listener_kinds: BTreeMap::new(),
		listener_transports: BTreeMap::new(),
		annotations: Vec::new(),
	}
}

/// Single-route graph: every connection terminates at
/// `Fetch(tag) -> WriteHttpResponse`. Used for `Require` mode.
fn graph_single_route(
	addr: SocketAddr,
	pki: &PkiFixture,
	client_auth: vane_core::rule::ClientAuthSpec,
	tag: &str,
) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));
	let meta = meta_with_client_auth(addr, pki.server_tls_cfg.clone(), client_auth);
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
			kind: FetchKind::HttpSynthesize,
			args: serde_json::json!({ "tag": tag }),
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	fetch.register(FetchKind::HttpSynthesize, tagged_fetch_factory);
	FlowGraph::link(sym, &mw, &fetch).expect("link mtls graph")
}

/// Branching graph: routes on `predicate` to one of two L7 fetches
/// `tag_match` / `tag_miss`. Used for `Request` + `tls.peer_cert.*`
/// predicate routing.
fn graph_branching_on_predicate(
	addr: SocketAddr,
	pki: &PkiFixture,
	client_auth: vane_core::rule::ClientAuthSpec,
	predicate: PredicateInst,
	tag_match: &str,
	tag_miss: &str,
) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));
	let meta = meta_with_client_auth(addr, pki.server_tls_cfg.clone(), client_auth);
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Check {
				predicate: PredicateId::new(0),
				on_match: NodeId::new(1),
				on_miss: NodeId::new(2),
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Upgrade { next: NodeId::new(3) }, // match
			Node::Upgrade { next: NodeId::new(4) }, // miss
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(5)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Fetch {
				id: FetchId::new(1),
				next_response: Some(NodeId::new(5)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![predicate],
		middlewares: vec![],
		fetches: vec![
			SymbolicFetchRef {
				kind: FetchKind::HttpSynthesize,
				args: serde_json::json!({ "tag": tag_match }),
				retry_buffer_required: false,
				allow_zero_rtt: None,
			},
			SymbolicFetchRef {
				kind: FetchKind::HttpSynthesize,
				args: serde_json::json!({ "tag": tag_miss }),
				retry_buffer_required: false,
				allow_zero_rtt: None,
			},
		],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	fetch.register(FetchKind::HttpSynthesize, tagged_fetch_factory);
	FlowGraph::link(sym, &mw, &fetch).expect("link mtls branch graph")
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

fn build_client_config(
	server_cert_pem: &str,
	with_client_auth: Option<(
		Vec<rustls_pki_types::CertificateDer<'static>>,
		rustls_pki_types::PrivateKeyDer<'static>,
	)>,
) -> rustls::ClientConfig {
	let mut roots = rustls::RootCertStore::empty();
	for cert in rustls_pemfile::certs(&mut server_cert_pem.as_bytes()) {
		roots.add(cert.expect("parse cert")).expect("add cert");
	}
	let builder = rustls::ClientConfig::builder().with_root_certificates(roots);
	let mut cfg = match with_client_auth {
		Some((chain, key)) => {
			builder.with_client_auth_cert(chain, key).expect("build client cert config")
		}
		None => builder.with_no_client_auth(),
	};
	cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
	cfg
}

async fn http_get_through_tls(
	addr: SocketAddr,
	cfg: rustls::ClientConfig,
) -> Result<(u16, Bytes), String> {
	let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
	let tcp = tokio::net::TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
	let server_name = rustls::pki_types::ServerName::try_from("localhost").expect("server name");
	let tls_stream = connector.connect(server_name, tcp).await.map_err(|e| e.to_string())?;
	let io = TokioIo::new(tls_stream);
	let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
		.await
		.map_err(|e| e.to_string())?;
	tokio::spawn(async move {
		let _ = conn.await;
	});
	let req = hyper::Request::builder()
		.method("GET")
		.uri("/")
		.header("host", "localhost")
		.body(Empty::<Bytes>::new())
		.expect("build GET");
	let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
	let status = resp.status().as_u16();
	let body = resp.into_body().collect().await.map_err(|e| e.to_string())?.to_bytes();
	Ok((status, body))
}

// Tests
#[tokio::test]
async fn mtls_require_accepts_valid_client_cert() {
	vane_engine::crypto::install_default_provider();
	let pki = rcgen_pki("ops-bot", &["ops-bot.internal"]);
	let trust_store = vane_core::rule::ClientTrustStoreConfig {
		ca_paths: vec![pki.ca_path.clone()],
		ca_dir: None,
		crls: vec![],
	};
	let addr = pick_port().await;
	let graph = graph_single_route(
		addr,
		&pki,
		vane_core::rule::ClientAuthSpec::Require { trust_store },
		"valid",
	);
	let (set, addr) = start_listener(graph).await;

	let cfg = build_client_config(
		&pki.ca_pem,
		Some((pki.client_cert_chain.clone(), pki.client_key.clone_key())),
	);
	let (status, body) = http_get_through_tls(addr, cfg).await.expect("client succeeds");
	assert_eq!(status, 200);
	assert_eq!(body.as_ref(), b"valid");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn mtls_require_rejects_handshake_without_client_cert() {
	vane_engine::crypto::install_default_provider();
	let pki = rcgen_pki("any-bot", &[]);
	let trust_store = vane_core::rule::ClientTrustStoreConfig {
		ca_paths: vec![pki.ca_path.clone()],
		ca_dir: None,
		crls: vec![],
	};
	let addr = pick_port().await;
	let graph = graph_single_route(
		addr,
		&pki,
		vane_core::rule::ClientAuthSpec::Require { trust_store },
		"valid",
	);
	let (set, addr) = start_listener(graph).await;

	// No client cert offered: handshake aborts. The client sees an
	// error from the rustls server (TLS alert or connection close).
	let cfg = build_client_config(&pki.ca_pem, None);
	let result = http_get_through_tls(addr, cfg).await;
	assert!(result.is_err(), "Require mode must reject cert-less client; got {result:?}");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn mtls_require_rejects_handshake_with_wrong_ca_client_cert() {
	vane_engine::crypto::install_default_provider();
	let pki = rcgen_pki("good-client", &[]);
	// Build a separate PKI whose CA the listener does NOT trust.
	let attacker = rcgen_pki("attacker", &[]);

	let trust_store = vane_core::rule::ClientTrustStoreConfig {
		ca_paths: vec![pki.ca_path.clone()],
		ca_dir: None,
		crls: vec![],
	};
	let addr = pick_port().await;
	let graph = graph_single_route(
		addr,
		&pki,
		vane_core::rule::ClientAuthSpec::Require { trust_store },
		"valid",
	);
	let (set, addr) = start_listener(graph).await;

	let cfg = build_client_config(
		&pki.ca_pem,
		Some((attacker.client_cert_chain.clone(), attacker.client_key.clone_key())),
	);
	let result = http_get_through_tls(addr, cfg).await;
	assert!(result.is_err(), "wrong-CA cert must be rejected at handshake; got {result:?}");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn mtls_request_routes_on_peer_cert_present() {
	vane_engine::crypto::install_default_provider();
	let pki = rcgen_pki("ops-bot", &["ops-bot.internal"]);
	let trust_store = vane_core::rule::ClientTrustStoreConfig {
		ca_paths: vec![pki.ca_path.clone()],
		ca_dir: None,
		crls: vec![],
	};
	let predicate = PredicateInst {
		path: FieldPath::TlsPeerCertPresent,
		op: CompiledOperator::Equals(CompiledValue::Bool(true)),
	};
	let addr = pick_port().await;
	let graph = graph_branching_on_predicate(
		addr,
		&pki,
		vane_core::rule::ClientAuthSpec::Request { trust_store },
		predicate,
		"with-cert",
		"without-cert",
	);
	let (set, addr) = start_listener(graph).await;

	// (a) Client with cert hits the `present == true` arm.
	let cfg_with = build_client_config(
		&pki.ca_pem,
		Some((pki.client_cert_chain.clone(), pki.client_key.clone_key())),
	);
	let (status, body) = http_get_through_tls(addr, cfg_with).await.expect("cert client succeeds");
	assert_eq!(status, 200);
	assert_eq!(body.as_ref(), b"with-cert");

	// (b) Client without cert hits the miss arm — Request mode lets
	// it through the handshake.
	let cfg_no = build_client_config(&pki.ca_pem, None);
	let (status, body) = http_get_through_tls(addr, cfg_no).await.expect("no-cert client succeeds");
	assert_eq!(status, 200);
	assert_eq!(body.as_ref(), b"without-cert");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn mtls_request_routes_on_peer_cert_subject_cn() {
	vane_engine::crypto::install_default_provider();
	let pki = rcgen_pki("ops-bot", &["ops-bot.internal"]);
	let trust_store = vane_core::rule::ClientTrustStoreConfig {
		ca_paths: vec![pki.ca_path.clone()],
		ca_dir: None,
		crls: vec![],
	};
	let predicate = PredicateInst {
		path: FieldPath::TlsPeerCertSubjectCn,
		op: CompiledOperator::Equals(CompiledValue::Str(Arc::from("ops-bot"))),
	};
	let addr = pick_port().await;
	let graph = graph_branching_on_predicate(
		addr,
		&pki,
		vane_core::rule::ClientAuthSpec::Request { trust_store },
		predicate,
		"with-cert",
		"without-cert",
	);
	let (set, addr) = start_listener(graph).await;

	let cfg = build_client_config(
		&pki.ca_pem,
		Some((pki.client_cert_chain.clone(), pki.client_key.clone_key())),
	);
	let (status, body) = http_get_through_tls(addr, cfg).await.expect("client succeeds");
	assert_eq!(status, 200);
	assert_eq!(body.as_ref(), b"with-cert", "subject_cn=ops-bot must hit the match arm");

	set.shutdown(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn mtls_request_routes_on_peer_cert_san_dns() {
	vane_engine::crypto::install_default_provider();
	let pki = rcgen_pki("svc-a", &["svc-a.internal", "svc-b.internal"]);
	let trust_store = vane_core::rule::ClientTrustStoreConfig {
		ca_paths: vec![pki.ca_path.clone()],
		ca_dir: None,
		crls: vec![],
	};
	let predicate = PredicateInst {
		path: FieldPath::TlsPeerCertSanDns,
		op: CompiledOperator::Contains(Bytes::from_static(b"svc-a.internal")),
	};
	let addr = pick_port().await;
	let graph = graph_branching_on_predicate(
		addr,
		&pki,
		vane_core::rule::ClientAuthSpec::Request { trust_store },
		predicate,
		"with-cert",
		"without-cert",
	);
	let (set, addr) = start_listener(graph).await;

	let cfg = build_client_config(
		&pki.ca_pem,
		Some((pki.client_cert_chain.clone(), pki.client_key.clone_key())),
	);
	let (status, body) = http_get_through_tls(addr, cfg).await.expect("client succeeds");
	assert_eq!(status, 200);
	assert_eq!(body.as_ref(), b"with-cert", "san_dns contains svc-a.internal must hit the match arm");

	set.shutdown(Duration::from_millis(500)).await;
}
