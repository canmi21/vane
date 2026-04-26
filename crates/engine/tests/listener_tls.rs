//! End-to-end tests for listener-side TLS termination.
//!
//! Builds a `SymbolicFlowGraph` whose meta carries a `listener_tls` entry
//! pointing at an rcgen-generated self-signed cert + key, links it through
//! `FlowGraph::link` (which parses PEM into `rustls::ServerConfig`), starts
//! the listener, and drives a real `tokio_rustls` client through the
//! handshake. The L7 sub-graph is the same `Upgrade -> Fetch -> Terminate`
//! shape used by `tests/hyper_upgrade.rs`; only the wire transport changes.
//!
//! Spec anchors:
//!
//! * `spec/architecture/08-tls.md` § _TLS termination (L4 → L7 upgrade)_ —
//!   the listener wraps the accepted `TcpStream` in a server-side rustls
//!   handshake before dispatching `L4Conn::Tls(Box<dyn AsyncReadWrite>)`.
//! * `spec/architecture/08-tls.md` § _ALPN_ — single-protocol ALPN this
//!   round; the server advertises `["http/1.1"]`.

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
use vane_core::{
	Body, ConnContext, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, FlowLogEvent, FlowLogSink,
	L7Fetch, L7FetchOutput, Node, NodeId, Request, Response, SymbolicFetchRef, SymbolicFlowGraph,
	Terminator, TerminatorId,
};
use vane_engine::ListenerSet;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
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

struct TlsFixture {
	_cert_file: NamedTempFile,
	_key_file: NamedTempFile,
	cert_pem: String,
	tls_cfg: vane_core::rule::TlsConfig,
}

fn rcgen_self_signed_for_localhost() -> TlsFixture {
	let issued =
		rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("self-signed cert");
	let cert_pem = issued.cert.pem();
	let key_pem = issued.signing_key.serialize_pem();

	let mut cert_file = NamedTempFile::new().expect("cert tmp");
	cert_file.write_all(cert_pem.as_bytes()).expect("write cert pem");
	let mut key_file = NamedTempFile::new().expect("key tmp");
	key_file.write_all(key_pem.as_bytes()).expect("write key pem");

	let tls_cfg = vane_core::rule::TlsConfig {
		cert_file: cert_file.path().to_path_buf(),
		key_file: key_file.path().to_path_buf(),
	};

	TlsFixture { _cert_file: cert_file, _key_file: key_file, cert_pem, tls_cfg }
}

/// Symbolic graph: `Upgrade -> Fetch(StaticOk) -> Terminate(WriteHttpResponse)`,
/// with `meta.listener_tls[addr] = tls_cfg`.
fn tls_static_ok_graph(addr: SocketAddr, tls_cfg: vane_core::rule::TlsConfig) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));

	let mut listener_tls = BTreeMap::new();
	listener_tls.insert(addr, tls_cfg);

	let meta = FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls,
	};

	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![
			Node::Upgrade { next: NodeId::new(1) },
			Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(2)),
				next_tunnel: None,
				collect_body_before: None,
			},
			Node::Terminate(TerminatorId::new(0)),
		],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![SymbolicFetchRef { kind: FetchKind::HttpSynthesize, args: Value::Null }],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});

	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	fetch.register(FetchKind::HttpSynthesize, |_args| Ok(FetchInst::L7(Arc::new(StaticOkFetch))));
	FlowGraph::link(sym, &mw, &fetch).expect("link tls graph")
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

async fn start_listener(graph: Arc<FlowGraph>) -> (ListenerSet, SocketAddr) {
	let addr = *graph.symbolic().entries.iter().next().expect("entries").0;
	let verbosity = Arc::new(VerbosityState::new());
	let sink: Arc<dyn FlowLogSink> = Arc::new(DropSink);
	let set = ListenerSet::new();
	set.start(Arc::new(ArcSwap::new(graph)), verbosity, sink);
	tokio::time::sleep(Duration::from_millis(50)).await;
	(set, addr)
}

fn build_client_config(server_cert_pem: &str, alpn: Vec<Vec<u8>>) -> rustls::ClientConfig {
	let mut roots = rustls::RootCertStore::empty();
	for cert in rustls_pemfile::certs(&mut server_cert_pem.as_bytes()) {
		roots.add(cert.expect("parse cert")).expect("add cert");
	}
	let mut cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
	cfg.alpn_protocols = alpn;
	cfg
}

#[tokio::test]
async fn tls_listener_completes_handshake_and_serves_h1_response() {
	vane_engine::crypto::install_default_provider();

	let fixture = rcgen_self_signed_for_localhost();
	let addr = pick_port().await;
	let graph = tls_static_ok_graph(addr, fixture.tls_cfg.clone());
	let (set, addr) = start_listener(graph).await;

	let client_cfg = build_client_config(&fixture.cert_pem, vec![b"http/1.1".to_vec()]);
	let connector = tokio_rustls::TlsConnector::from(Arc::new(client_cfg));

	let tcp = tokio::net::TcpStream::connect(addr).await.expect("client tcp connect");
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
	let resp = sender.send_request(req).await.expect("send_request");
	assert_eq!(resp.status().as_u16(), 200, "TLS-wrapped H1 GET must yield 200");

	let body = resp.into_body().collect().await.expect("collect").to_bytes();
	assert_eq!(body.as_ref(), b"ok");

	set.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn tls_listener_drops_invalid_handshake() {
	vane_engine::crypto::install_default_provider();

	let fixture = rcgen_self_signed_for_localhost();
	let addr = pick_port().await;
	let graph = tls_static_ok_graph(addr, fixture.tls_cfg);
	let (set, addr) = start_listener(graph).await;

	// Connect raw TCP and send bytes that aren't a valid ClientHello. The
	// listener's TlsAcceptor must reject the handshake and close the
	// connection — we observe EOF on read.
	let mut tcp = tokio::net::TcpStream::connect(addr).await.expect("client tcp connect");
	tcp.write_all(b"this is not a TLS ClientHello\n").await.expect("write garbage");

	let mut buf = vec![0u8; 64];
	let read = tokio::time::timeout(Duration::from_secs(2), tcp.read(&mut buf))
		.await
		.expect("server must close — no hang");
	let n = read.unwrap_or(0);
	// Server closes either with TLS alert bytes or a clean EOF (n == 0).
	// In neither case should we observe the L7 path's "ok" body.
	assert!(!buf[..n].windows(2).any(|w| w == b"ok"), "L7 fetch must not run on bad handshake");

	set.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn tls_listener_alpn_negotiation_returns_http_1_1() {
	vane_engine::crypto::install_default_provider();

	let fixture = rcgen_self_signed_for_localhost();
	let addr = pick_port().await;
	let graph = tls_static_ok_graph(addr, fixture.tls_cfg.clone());
	let (set, addr) = start_listener(graph).await;

	// Client offers both `h2` and `http/1.1`; the server only advertises
	// `http/1.1`, so the negotiated protocol must be `http/1.1`.
	let client_cfg =
		build_client_config(&fixture.cert_pem, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
	let connector = tokio_rustls::TlsConnector::from(Arc::new(client_cfg));

	let tcp = tokio::net::TcpStream::connect(addr).await.expect("client tcp connect");
	let server_name = rustls::pki_types::ServerName::try_from("localhost").expect("server name");
	let tls_stream = connector.connect(server_name, tcp).await.expect("tls handshake");

	let alpn = tls_stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
	assert_eq!(
		alpn,
		Some(b"http/1.1".to_vec()),
		"server-side ALPN must pick http/1.1 even when client also offers h2",
	);

	drop(tls_stream);
	set.shutdown(Duration::from_secs(2)).await;
}
