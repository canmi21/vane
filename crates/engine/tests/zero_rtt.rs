//! End-to-end tests for TLS 1.3 0-RTT (early data).
//!
//! Drives a `tokio_rustls` client (built with the `early-data` feature)
//! through two handshakes against the same listener:
//!
//! 1. The first connection completes a full handshake. The server emits
//!    a `NewSessionTicket` with the early-data permission extension
//!    (because `ServerConfig.max_early_data_size > 0`). The client's
//!    in-memory session cache stores the ticket.
//! 2. The second connection enables 0-RTT on the connector.
//!    `tokio_rustls` enters its `EarlyData` state when the cached
//!    session permits early data, and bytes written before the
//!    handshake completes are sent as TLS 1.3 early data.
//!
//! What the tests prove:
//!
//! * `enable_zero_rtt: true` actually wires the rustls server-side
//!   acceptance (the ticketer is skipped per `08-tls.md` § _Exception:
//!   0-RTT-enabled listeners_, `max_early_data_size = 16 KiB`, and
//!   per-`ServerConfig` `ServerSessionMemoryCache` is the stateful
//!   resumption store rustls 0.23 requires for 0-RTT).
//! * `run_tls`'s early-data drain (per the wiring committed earlier)
//!   actually fires: the request bytes that arrived as 0-RTT are seen
//!   by the L7 path.
//! * The per-rule `allow_zero_rtt: false` runtime gate at `Node::Fetch`
//!   actually synthesizes a 425 Too Early response with
//!   `Cache-Control: no-store`, leaving the connection open so a
//!   well-behaved client can retry.
//! * Ordinary 1-RTT requests on a 0-RTT-enabled listener are unaffected
//!   (the dormant 0-RTT state must not regress the high-traffic path).
//!
//! Spec anchors: `spec/architecture/08-tls.md` § _TLS 1.3 0-RTT (early
//! data)_, § _Session ticket rotation_ § _Exception: 0-RTT-enabled
//! listeners_.

#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::BodyExt as _;
use parking_lot::Mutex;
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
		// Listener-side opt-in flips `max_early_data_size = 16 KiB`
		// and (per Step 1 of this PR) skips the daemon-wide ticketer
		// so rustls's per-`ServerConfig` session storage becomes the
		// stateful resumption backend.
		enable_zero_rtt: true,
		ocsp_path: None,
		ocsp_fetch: false,
	};
	TlsFixture { _cert_file: cert_file, _key_file: key_file, cert_pem, tls_cfg }
}

/// Fetch that captures the request body bytes (if any) into a shared
/// slot and replies 200 with a short static body so test assertions can
/// inspect both the wire response *and* what reached the application.
struct CaptureBodyFetch {
	captured: Arc<Mutex<Vec<u8>>>,
}

#[async_trait]
impl L7Fetch for CaptureBodyFetch {
	async fn fetch(
		&self,
		req: Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		let collected =
			req.into_body().collect().await.map_err(|e| Error::io(format!("collect body: {e}")))?;
		let bytes = collected.to_bytes();
		*self.captured.lock() = bytes.to_vec();
		let resp: Response = http::Response::builder()
			.status(200)
			.body(Body::Static(Bytes::from_static(b"ok")))
			.expect("build response");
		Ok(L7FetchOutput::Response(resp))
	}
}

/// Build a graph whose listener has `enable_zero_rtt = true` and whose
/// single fetch's `allow_zero_rtt` is the test's choice. The hand-built
/// graph skips compile/lower (which would otherwise enforce the GET-only
/// idempotency check), so the test can drive the runtime gate
/// independently.
fn tls_zero_rtt_graph(
	addr: SocketAddr,
	tls_cfg: vane_core::rule::TlsConfig,
	allow_zero_rtt: Option<bool>,
	captured: &Arc<Mutex<Vec<u8>>>,
) -> Arc<FlowGraph> {
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
			enable_zero_rtt: true,
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
			allow_zero_rtt,
		}],
		terminators: vec![Terminator::WriteHttpResponse],
		entries,
		meta,
	});

	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	let captured_for_factory = Arc::clone(captured);
	fetch.register(FetchKind::HttpSynthesize, move |_args| {
		Ok(FetchInst::L7(Arc::new(CaptureBodyFetch { captured: Arc::clone(&captured_for_factory) })))
	});
	FlowGraph::link(sym, &mw, &fetch).expect("link 0-rtt graph")
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

/// Build a `ClientConfig` with `enable_early_data = true` and an
/// in-memory resumption cache. The same `Arc<ClientConfig>` is reused
/// across the two test handshakes so the cache survives.
fn build_client_config(server_cert_pem: &str) -> rustls::ClientConfig {
	let mut roots = rustls::RootCertStore::empty();
	for cert in rustls_pemfile::certs(&mut server_cert_pem.as_bytes()) {
		roots.add(cert.expect("parse cert")).expect("add cert");
	}
	let mut cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
	cfg.resumption = rustls::client::Resumption::default();
	cfg.enable_early_data = true;
	cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
	cfg
}

/// Drive one full TLS handshake plus a raw HTTP/1.1 GET round-trip.
/// Reads the response to EOF (`Connection: close`) so the server's
/// post-handshake `NewSessionTicket` record is consumed by the client's
/// rustls state and stored in the resumption cache before this returns.
async fn warmup_session_via_full_handshake(
	addr: SocketAddr,
	cfg: Arc<rustls::ClientConfig>,
) -> rustls::HandshakeKind {
	let connector = tokio_rustls::TlsConnector::from(cfg);
	let tcp = tokio::net::TcpStream::connect(addr).await.expect("client tcp connect");
	let server_name = rustls::pki_types::ServerName::try_from("localhost").expect("server name");
	let mut tls_stream = connector.connect(server_name, tcp).await.expect("tls handshake");
	let kind =
		tls_stream.get_ref().1.handshake_kind().expect("handshake_kind populated after handshake");
	tls_stream
		.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
		.await
		.expect("write GET");
	let mut response = Vec::new();
	tls_stream.read_to_end(&mut response).await.expect("read to EOF");
	assert!(response.starts_with(b"HTTP/1.1 200"), "warmup must yield 200: {response:?}");
	kind
}

/// Open a 0-RTT-enabled connection: writes `request` *before* awaiting
/// handshake completion. Returns the negotiated handshake kind plus the
/// fully accumulated response (no `Connection: close` so the same
/// stream can serve a follow-up request).
struct ZeroRttResult {
	handshake_kind: rustls::HandshakeKind,
	early_data_was_accepted: bool,
	response: Vec<u8>,
	stream: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
}

async fn drive_zero_rtt_request(
	addr: SocketAddr,
	cfg: Arc<rustls::ClientConfig>,
	request: &[u8],
) -> ZeroRttResult {
	let connector = tokio_rustls::TlsConnector::from(cfg).early_data(true);
	let tcp = tokio::net::TcpStream::connect(addr).await.expect("client tcp connect");
	let server_name = rustls::pki_types::ServerName::try_from("localhost").expect("server name");
	let mut tls_stream = connector.connect(server_name, tcp).await.expect("tls handshake");
	// Write the request — when tokio-rustls is in `EarlyData` state
	// (cached session permits 0-RTT and `enable_early_data = true`),
	// these bytes become TLS 1.3 early data.
	tls_stream.write_all(request).await.expect("write request");
	tls_stream.flush().await.expect("flush");
	// Read until we see at least the response headers + the static
	// "ok" body terminator. We don't use `Connection: close` here so
	// the same stream remains usable for follow-up requests.
	let response = read_one_http1_response(&mut tls_stream).await;
	let (handshake_kind, early_data_was_accepted) = {
		let (_io, sess) = tls_stream.get_ref();
		(
			sess.handshake_kind().expect("handshake_kind populated after handshake"),
			sess.is_early_data_accepted(),
		)
	};
	ZeroRttResult { handshake_kind, early_data_was_accepted, response, stream: tls_stream }
}

/// Read one HTTP/1.1 response off the TLS stream. Stops once the
/// content-length-declared body has been fully read (or, for empty
/// bodies, once the headers terminate with `\r\n\r\n`).
async fn read_one_http1_response(
	stream: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
) -> Vec<u8> {
	let mut buf = Vec::with_capacity(512);
	let mut tmp = [0u8; 1024];
	let mut headers_end: Option<usize> = None;
	let mut content_length: usize = 0;
	loop {
		let n = stream.read(&mut tmp).await.expect("read response");
		if n == 0 {
			break;
		}
		buf.extend_from_slice(&tmp[..n]);
		if headers_end.is_none()
			&& let Some(pos) = find_header_end(&buf)
		{
			headers_end = Some(pos);
			content_length = parse_content_length(&buf[..pos]).unwrap_or(0);
		}
		if let Some(pos) = headers_end
			&& buf.len() >= pos + content_length
		{
			break;
		}
	}
	buf
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
	buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
	let s = std::str::from_utf8(headers).ok()?;
	for line in s.split("\r\n") {
		if let Some(rest) =
			line.strip_prefix("Content-Length:").or_else(|| line.strip_prefix("content-length:"))
		{
			return rest.trim().parse::<usize>().ok();
		}
	}
	None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 0-RTT GET on a rule with `allow_zero_rtt: Some(true)` must succeed.
/// The client's second handshake is `Resumed` and the server-side
/// `is_early_data_accepted()` is true.
#[tokio::test]
async fn zero_rtt_get_accepted_when_rule_allows() {
	vane_engine::crypto::install_default_provider();

	let fixture = rcgen_self_signed_for_localhost();
	let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
	let addr = pick_port().await;
	let graph = tls_zero_rtt_graph(addr, fixture.tls_cfg.clone(), Some(true), &captured);
	let (set, addr) = start_listener(graph).await;

	let client_cfg = Arc::new(build_client_config(&fixture.cert_pem));

	let warmup = warmup_session_via_full_handshake(addr, Arc::clone(&client_cfg)).await;
	assert_eq!(warmup, rustls::HandshakeKind::Full, "warmup must be a full handshake");

	let result = drive_zero_rtt_request(
		addr,
		Arc::clone(&client_cfg),
		b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
	)
	.await;

	assert_eq!(
		result.handshake_kind,
		rustls::HandshakeKind::Resumed,
		"second handshake must resume off the cached ticket",
	);
	assert!(
		result.early_data_was_accepted,
		"server must have accepted early data — 0-RTT path is dormant otherwise",
	);
	assert!(
		result.response.starts_with(b"HTTP/1.1 200"),
		"0-RTT GET must yield 200 when allow_zero_rtt: true: {:?}",
		String::from_utf8_lossy(&result.response),
	);

	drop(result.stream);
	set.shutdown(Duration::from_millis(500)).await;
}

/// 0-RTT GET on a rule with `allow_zero_rtt: Some(false)` must receive
/// the synthetic 425 Too Early. The connection stays up — a follow-up
/// 1-RTT request on the same stream succeeds.
#[tokio::test]
async fn zero_rtt_rejected_with_425_when_rule_disallows() {
	vane_engine::crypto::install_default_provider();

	let fixture = rcgen_self_signed_for_localhost();
	let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
	let addr = pick_port().await;
	let graph = tls_zero_rtt_graph(addr, fixture.tls_cfg.clone(), Some(false), &captured);
	let (set, addr) = start_listener(graph).await;

	let client_cfg = Arc::new(build_client_config(&fixture.cert_pem));

	let warmup = warmup_session_via_full_handshake(addr, Arc::clone(&client_cfg)).await;
	assert_eq!(warmup, rustls::HandshakeKind::Full, "warmup must be a full handshake");

	let result = drive_zero_rtt_request(
		addr,
		Arc::clone(&client_cfg),
		b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
	)
	.await;

	assert_eq!(
		result.handshake_kind,
		rustls::HandshakeKind::Resumed,
		"second handshake must resume off the cached ticket",
	);
	assert!(
		result.early_data_was_accepted,
		"server still accepts the early-data bytes; the 425 is synthesised by the L7 gate above rustls",
	);
	assert!(
		result.response.starts_with(b"HTTP/1.1 425"),
		"runtime gate must synthesise 425 Too Early on allow_zero_rtt: false: {:?}",
		String::from_utf8_lossy(&result.response),
	);
	let header_block = lowercase_header_block(&result.response);
	assert!(
		header_block.contains("cache-control: no-store"),
		"425 must carry Cache-Control: no-store per spec § _Runtime flow_: {header_block}",
	);

	// The connection must stay up — write a follow-up 1-RTT request
	// on the same TLS stream and expect a normal 200 (the runtime
	// gate fires per-request, not per-connection, but the synthesised
	// 425 path still arrives via 0-RTT — so the second request is the
	// one that proves the connection itself wasn't closed).
	let mut stream = result.stream;
	stream
		.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
		.await
		.expect("write follow-up GET");
	let mut follow_up = Vec::new();
	stream.read_to_end(&mut follow_up).await.expect("read follow-up");
	// The follow-up arrives purely as 1-RTT (the early-data buffer
	// was drained by the first request) and must hit the L7 path
	// normally — i.e. the 425 gate must be scoped to the request
	// whose bytes actually arrived as 0-RTT, not to the connection
	// for its entire lifetime.
	assert!(
		follow_up.starts_with(b"HTTP/1.1 200"),
		"follow-up 1-RTT request on the same connection must yield 200; the 425 gate must be per-request not per-connection: {:?}",
		String::from_utf8_lossy(&follow_up),
	);

	set.shutdown(Duration::from_millis(500)).await;
}

fn lowercase_header_block(response: &[u8]) -> String {
	let s = std::str::from_utf8(response).unwrap_or("");
	let end = s.find("\r\n\r\n").unwrap_or(s.len());
	s[..end].to_ascii_lowercase()
}

/// 0-RTT GET with a small body on a rule that allows 0-RTT must still
/// complete (200) and the body bytes must reach the L7 fetch. Per
/// `08-tls.md` § _Hardcoded limits_, the body is architecturally
/// "downgraded" to 1-RTT (`tokio_rustls::into_stream` returns only after
/// the handshake completes, so the bytes are processed alongside any
/// post-handshake 1-RTT data).
#[tokio::test]
async fn zero_rtt_get_with_body_downgrades_and_completes() {
	vane_engine::crypto::install_default_provider();

	let fixture = rcgen_self_signed_for_localhost();
	let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
	let addr = pick_port().await;
	let graph = tls_zero_rtt_graph(addr, fixture.tls_cfg.clone(), Some(true), &captured);
	let (set, addr) = start_listener(graph).await;

	let client_cfg = Arc::new(build_client_config(&fixture.cert_pem));

	let warmup = warmup_session_via_full_handshake(addr, Arc::clone(&client_cfg)).await;
	assert_eq!(warmup, rustls::HandshakeKind::Full, "warmup must be a full handshake");

	let body = b"hello-zero-rtt-body";
	let request = format!(
		"GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
		body.len(),
		std::str::from_utf8(body).expect("ascii body"),
	);
	let result = drive_zero_rtt_request(addr, Arc::clone(&client_cfg), request.as_bytes()).await;

	assert_eq!(result.handshake_kind, rustls::HandshakeKind::Resumed);
	assert!(
		result.early_data_was_accepted,
		"server must accept early data; the body downgrade is architectural, not a rejection",
	);
	assert!(
		result.response.starts_with(b"HTTP/1.1 200"),
		"GET-with-body via 0-RTT must complete 200: {:?}",
		String::from_utf8_lossy(&result.response),
	);
	assert_eq!(
		captured.lock().as_slice(),
		body,
		"body must reach the L7 fetch — early-data drain + 1-RTT continuation must concatenate",
	);

	drop(result.stream);
	set.shutdown(Duration::from_millis(500)).await;
}

/// A 1-RTT request on a `enable_zero_rtt: true` listener must not
/// regress. Same listener config as the 0-RTT tests, but the client
/// never enables early data on the connector — the request rides a
/// vanilla full handshake.
#[tokio::test]
async fn one_rtt_request_unaffected_on_zero_rtt_listener() {
	vane_engine::crypto::install_default_provider();

	let fixture = rcgen_self_signed_for_localhost();
	let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
	let addr = pick_port().await;
	let graph = tls_zero_rtt_graph(addr, fixture.tls_cfg.clone(), Some(true), &captured);
	let (set, addr) = start_listener(graph).await;

	let client_cfg = Arc::new(build_client_config(&fixture.cert_pem));
	let kind = warmup_session_via_full_handshake(addr, Arc::clone(&client_cfg)).await;
	assert_eq!(
		kind,
		rustls::HandshakeKind::Full,
		"plain client connect must complete a full 1-RTT handshake — 0-RTT must not be silently engaged",
	);

	set.shutdown(Duration::from_millis(500)).await;
}
