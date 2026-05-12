//! End-to-end coverage for `fetch::upstream::dial_upstream` against a
//! real TLS upstream, plus an `HttpProxyFetch` round-trip that
//! exercises the full hyper handshake over the rustls connection.
//!
//! Each test stands up a `tokio_rustls::TlsAcceptor` on an ephemeral
//! port using an rcgen self-signed certificate, drives the
//! engine-side dial path with `insecure_skip_verify: true`, and
//! asserts a clean byte exchange / HTTP response. The fixture mirrors
//! the pattern in `tests/listener_tls.rs` but the server-side
//! certificate is consumed by the engine's *upstream* path here.

use std::sync::Arc;

use http_body_util::BodyExt;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use vane_core::{
	Body, ConnContext, FlowCtx, FlowLogEvent, FlowLogSink, HttpVersion, NodeId, TlsInfo,
	TrajectoryBuilder, Transport,
};
use vane_engine::fetch::http_proxy::factory as http_proxy_factory;
use vane_engine::fetch::upstream::{UpstreamTls, build_client_config, dial_upstream};
use vane_engine::flow_graph::FetchInst;
use vane_engine::verbosity::VerbosityState;

/// rcgen fixture: build a self-signed cert+key for `localhost` and a
/// matching `rustls::ServerConfig` ready to feed into `TlsAcceptor`.
fn rcgen_server_config() -> Arc<rustls::ServerConfig> {
	let issued =
		rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("self-signed cert");
	let cert_der: CertificateDer<'static> = issued.cert.der().clone();
	let key_der: PrivateKeyDer<'static> =
		PrivateKeyDer::Pkcs8(issued.signing_key.serialize_der().into());
	let cfg = rustls::ServerConfig::builder()
		.with_no_client_auth()
		.with_single_cert(vec![cert_der], key_der)
		.expect("build server config");
	Arc::new(cfg)
}

/// `insecure_skip_verify: true` [`UpstreamTls`] bound to `localhost`.
fn skip_verify_tls() -> UpstreamTls {
	use vane_engine::fetch::client_cache::{RootCaSource, TlsConfigFingerprint, VerifyMode};
	let client_config = build_client_config(true).expect("build insecure client config");
	let fingerprint = TlsConfigFingerprint {
		root_ca: RootCaSource::Skip,
		client_cert_hash: None,
		crl_sources: Vec::new(),
		verify_mode: VerifyMode::Skip,
		alpn_protocols: Vec::new(),
	};
	UpstreamTls {
		client_config,
		verify_hostname: "localhost".to_string(),
		fingerprint,
		crls: Vec::new(),
		client_cert: None,
	}
}

/// Binds an ephemeral TLS server, accepts one connection, runs
/// `handler` over the established TLS stream, then returns the chosen
/// address. Caller awaits the join handle to let the server task
/// complete naturally before the test exits.
async fn spawn_tls_echo() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
	vane_engine::crypto::install_default_provider();
	vane_testutil::allow_insecure_upstream_for_tests();
	let server_config = rcgen_server_config();
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local_addr");
	let acceptor = TlsAcceptor::from(server_config);
	let handle = tokio::spawn(async move {
		let (sock, _) = listener.accept().await.expect("accept");
		let mut tls = acceptor.accept(sock).await.expect("server tls handshake");
		let mut buf = [0u8; 5];
		tls.read_exact(&mut buf).await.expect("read");
		tls.write_all(&buf).await.expect("write echo");
	});
	(addr, handle)
}

#[tokio::test]
async fn dial_upstream_completes_tls_handshake_with_insecure_skip_verify() {
	let (addr, server_task) = spawn_tls_echo().await;
	let tls = skip_verify_tls();
	let mut conn = dial_upstream(
		&addr.to_string(),
		Some(&tls),
		&tokio_util::sync::CancellationToken::new(),
		vane_engine::fetch::upstream::DEFAULT_DIAL_TIMEOUT,
	)
	.await
	.expect("dial");
	conn.write_all(b"hello").await.expect("client write");
	let mut buf = [0u8; 5];
	conn.read_exact(&mut buf).await.expect("client read");
	assert_eq!(&buf, b"hello", "tls echo round-trip");
	server_task.await.expect("server task");
}

/// Minimal HTTPS service: accept one TLS connection, drive a
/// `hyper::server::conn::http1` handshake against it, respond with
/// the configured body. Used by the [`HttpProxyFetch`] round-trip test.
async fn spawn_https_static(
	body: &'static str,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
	vane_engine::crypto::install_default_provider();
	vane_testutil::allow_insecure_upstream_for_tests();
	let server_config = rcgen_server_config();
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local_addr");
	let acceptor = TlsAcceptor::from(server_config);
	let handle = tokio::spawn(async move {
		let (sock, _) = listener.accept().await.expect("accept");
		let tls = acceptor.accept(sock).await.expect("tls handshake");
		let io = TokioIo::new(tls);
		let svc = hyper::service::service_fn(move |_req: hyper::Request<hyper::body::Incoming>| {
			let resp_body = body.to_string();
			async move {
				Ok::<_, std::convert::Infallible>(
					hyper::Response::builder()
						.status(200)
						.header("content-type", "text/plain")
						.body(http_body_util::Full::new(Bytes::from(resp_body)))
						.expect("build response"),
				)
			}
		});
		let _ = hyper::server::conn::http1::Builder::new().serve_connection(io, svc).await;
	});
	(addr, handle)
}

/// Minimal `FlowLogSink` that drops every event. The fetch path emits
/// a few diagnostic logs but doesn't drive the test — keeping the
/// sink trivial keeps the fixture small.
struct NullSink;
impl FlowLogSink for NullSink {
	fn emit(&self, _event: FlowLogEvent) {}
}

fn make_ctx_and_conn() -> (Arc<ConnContext>, FlowCtx) {
	let conn = Arc::new(ConnContext {
		id: vane_core::ConnId(1),
		remote: "127.0.0.1:0".parse().unwrap(),
		local: "127.0.0.1:0".parse().unwrap(),
		transport: Transport::Tcp,
		entered_at: std::time::Instant::now(),
		tls: Mutex::new(Some(TlsInfo {
			sni: None,
			alpn: None,
			version: None,
			peer_cert: None,
			zero_rtt_used: false,
		})),
		http_version: std::sync::OnceLock::from(HttpVersion::Http1_1),
		user: Mutex::new(http::Extensions::new()),
	});
	let span = tracing::info_span!("test");
	let ctx = FlowCtx {
		span,
		log: Arc::new(NullSink) as Arc<dyn FlowLogSink>,
		cancel: tokio_util::sync::CancellationToken::new(),
		accept_cancel: tokio_util::sync::CancellationToken::new(),
		verbosity: VerbosityState::new().current(),
		trajectory: TrajectoryBuilder::new(conn.id, NodeId::new(0), 0),
	};
	(conn, ctx)
}

#[tokio::test]
async fn http_proxy_factory_with_tls_routes_https_request_round_trip() {
	let (addr, server_task) = spawn_https_static("hello-from-https").await;
	let factory_args = serde_json::json!({
		"upstream": addr.to_string(),
		"tls": { "insecure_skip_verify": true, "verify_hostname": "localhost" },
	});
	let inst = http_proxy_factory(&factory_args, None).expect("factory");
	let FetchInst::L7(fetch) = inst else {
		panic!("expected L7 fetch");
	};

	let (conn, mut ctx) = make_ctx_and_conn();
	let req = http::Request::builder()
		.uri("http://placeholder/path?q=1")
		.body(Body::Empty)
		.expect("build request");
	let outcome = fetch.fetch(req, &conn, &mut ctx).await.expect("fetch");
	let vane_core::L7FetchOutput::Response(resp) = outcome else {
		panic!("expected Response from HttpProxyFetch");
	};
	assert_eq!(resp.status(), 200, "upstream HTTPS responded 200");

	// Drain the streaming body to verify the TLS connection delivered
	// the full response payload through the IncomingAdapter.
	let Body::Stream(s) = resp.into_body() else {
		panic!("expected streaming body");
	};
	let collected = s.collect().await.expect("collect stream").to_bytes();
	assert_eq!(&collected[..], b"hello-from-https");
	// The pooled `hyper_util::client::legacy::Client` keeps the
	// upstream connection alive for keep-alive reuse, so the test
	// fixture's single-accept server never sees a clean close from
	// the client. Drop the fetch (releases the pool's handle on the
	// connection) and abort the server task instead of awaiting it.
	drop(fetch);
	server_task.abort();
}
