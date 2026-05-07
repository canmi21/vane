//! End-to-end test for the daemon-wide TLS session ticketer.
//!
//! Drives two TLS handshakes through the same `rustls::ClientConfig`
//! (whose `Resumption` is backed by an in-memory session cache).
//! After the first handshake completes a full HTTP/1.1
//! request/response cycle, the server has sent its post-handshake
//! `NewSessionTicket` and the client cache holds it. The second
//! handshake to the same listener observes
//! `HandshakeKind::Resumed`, proving that
//! `vane_engine::tls::install_default_ticketer` actually wired
//! `ServerConfig.ticketer` into the listener.
//!
//! Spec anchor: `spec/crates/engine-tls.md` § _Session ticket
//! rotation_.

#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
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
		sni: None,
		cert_file: Some(cert_file.path().to_path_buf()),
		key_file: Some(key_file.path().to_path_buf()),
		managed: None,
		client_auth: None,
		enable_zero_rtt: false,
		ocsp_path: None,
		ocsp_fetch: false,
	};
	TlsFixture { _cert_file: cert_file, _key_file: key_file, cert_pem, tls_cfg }
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

fn tls_static_ok_graph(addr: SocketAddr, tls_cfg: vane_core::rule::TlsConfig) -> Arc<FlowGraph> {
	let mut entries = HashMap::new();
	entries.insert(addr, NodeId::new(0));

	let mut listener_tls = BTreeMap::new();
	listener_tls.insert(
		addr,
		vane_core::rule::ListenerTlsSpec {
			default: Some(tls_cfg),
			sni_certs: BTreeMap::new(),
			managed_snis: BTreeMap::new(),
			client_auth: vane_core::rule::ClientAuthSpec::None,
			enable_zero_rtt: false,
		},
	);

	let meta = FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: BTreeMap::new(),
		listener_tls,
		listener_kinds: BTreeMap::new(),

		listener_transports: BTreeMap::new(),
		annotations: Vec::new(),
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
			kind: FetchKind::HttpSynthesize,
			args: Value::Null,
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});

	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	fetch.register(FetchKind::HttpSynthesize, |_args| Ok(FetchInst::L7(Arc::new(StaticOkFetch))));
	FlowGraph::link(sym, &mw, &fetch).expect("link tls graph")
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

fn build_client_config_with_resumption(server_cert_pem: &str) -> rustls::ClientConfig {
	let mut roots = rustls::RootCertStore::empty();
	for cert in rustls_pemfile::certs(&mut server_cert_pem.as_bytes()) {
		roots.add(cert.expect("parse cert")).expect("add cert");
	}
	let mut cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
	// `Resumption::default()` => `in_memory_sessions(256)`. Smaller
	// values pass through `ClientSessionMemoryCache::new(size)` whose
	// per-server slot count is `(size + 7) / 8` — calling it with
	// `size <= 8` collapses to a 1-server cache that evicts the
	// just-inserted entry, silently disabling resumption.
	cfg.resumption = rustls::client::Resumption::default();
	cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
	cfg
}

#[tokio::test]
async fn second_tls_handshake_resumes_via_shared_ticketer() {
	vane_engine::crypto::install_default_provider();
	vane_engine::tls::install_default_ticketer().expect("install ticketer");
	assert!(
		vane_engine::tls::default_ticketer().is_some(),
		"OnceLock must be populated before linking the graph",
	);

	let fixture = rcgen_self_signed_for_localhost();
	let addr = pick_port().await;
	let graph = tls_static_ok_graph(addr, fixture.tls_cfg.clone());
	let (set, addr) = start_listener(graph).await;

	let client_cfg = Arc::new(build_client_config_with_resumption(&fixture.cert_pem));
	let connector = tokio_rustls::TlsConnector::from(Arc::clone(&client_cfg));

	// First handshake: establishes a fresh session and lets the server
	// send its NewSessionTicket. `Connection: close` means the client's
	// `read_to_end` runs until the server hangs up, so every record
	// the server emits — including post-handshake tickets — is
	// processed by rustls and stored in the in-memory session cache
	// before the call returns.
	let first = drive_handshake(addr, &connector).await;
	assert_eq!(
		first,
		rustls::HandshakeKind::Full,
		"first handshake against a fresh session cache must be a full handshake",
	);

	// Second handshake: same `ClientConfig` (and therefore the same
	// in-memory session store) so the cached ticket is offered, and
	// the server — backed by the daemon-wide ticketer — accepts it
	// and resumes.
	let second = drive_handshake(addr, &connector).await;
	assert_eq!(
		second,
		rustls::HandshakeKind::Resumed,
		"second handshake must resume via the daemon-wide ticketer",
	);

	set.shutdown(Duration::from_millis(500)).await;
}

/// Drive one TLS handshake + raw HTTP/1.1 GET round-trip, returning
/// the handshake kind. Reads the server's response to EOF
/// (`Connection: close`) so any post-handshake `NewSessionTicket`
/// records get processed by rustls and stored in the client's
/// session cache before the function returns.
async fn drive_handshake(
	addr: SocketAddr,
	connector: &tokio_rustls::TlsConnector,
) -> rustls::HandshakeKind {
	let tcp = tokio::net::TcpStream::connect(addr).await.expect("client tcp connect");
	let server_name = rustls::pki_types::ServerName::try_from("localhost").expect("server name");
	let mut tls_stream = connector.connect(server_name, tcp).await.expect("tls handshake");

	// `handshake_kind` is populated once the handshake completes —
	// captured here so the same value is observable irrespective of
	// what the request/response side does next.
	let kind =
		tls_stream.get_ref().1.handshake_kind().expect("handshake_kind populated after handshake");

	tls_stream
		.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
		.await
		.expect("write request");
	let mut response = Vec::new();
	tls_stream.read_to_end(&mut response).await.expect("read to EOF");
	assert!(response.starts_with(b"HTTP/1.1 200"), "server must serve 200: {response:?}");
	assert!(response.ends_with(b"ok"), "response must terminate in 'ok' body");

	kind
}
