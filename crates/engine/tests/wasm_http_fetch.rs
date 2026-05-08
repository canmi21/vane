//! End-to-end coverage for `HyperHttpFetchBackend`. Spawns a local
//! cleartext echo server and drives requests through the backend the
//! way `vane-wasm`'s `http_fetch_core` would. Exercises:
//!
//! * Happy path 200 with response body collection.
//! * `BodyTooLarge` enforcement when the response exceeds
//!   `limits.max_body_bytes`.
//! * `Timeout` enforcement when the upstream hangs longer than
//!   `limits.timeout_ms`.
//! * Redirect-follow honouring `limits.follow_redirects` plus
//!   method-rewrite on 301/302/303 and preservation on 307/308.
//! * TLS posture switching: a self-signed-cert upstream is only
//!   reachable when `limits.allow_insecure == true`.

use std::convert::Infallible;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tempfile::NamedTempFile;
use vane_core::{HttpFetchBackend, HttpFetchError, HttpFetchLimits, HttpFetchRequest};
use vane_engine::wasm_fetch::HyperHttpFetchBackend;

/// Cleartext echo server. Returns a body whose size is set by the
/// `x-body-size` request header (default 2 bytes "ok"). When
/// `x-sleep-ms` is present, sleeps for that many ms before responding
/// — used by the timeout test.
async fn spawn_echo_server() -> SocketAddr {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind echo");
	let addr = listener.local_addr().expect("local_addr");
	tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			tokio::spawn(async move {
				let io = TokioIo::new(sock);
				let _ =
					hyper::server::conn::http1::Builder::new().serve_connection(io, service_fn(serve)).await;
			});
		}
	});
	addr
}

async fn serve(
	req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
	// Path-based routing so that headers preserved across redirects
	// don't loop the server: only `/start` honours `x-redirect-to`,
	// the second-hop request lands on `/landing` and falls through to
	// `/method-echo` (or the default size-echo handler).
	let path = req.uri().path().to_string();
	if path == "/start"
		&& let Some(target) = req.headers().get("x-redirect-to").and_then(|v| v.to_str().ok())
	{
		let status = req
			.headers()
			.get("x-redirect-status")
			.and_then(|v| v.to_str().ok())
			.and_then(|s| s.parse::<u16>().ok())
			.unwrap_or(302);
		return Ok(
			hyper::Response::builder()
				.status(status)
				.header("location", target)
				.body(Full::<Bytes>::new(Bytes::new()))
				.expect("build redirect"),
		);
	}
	if path == "/landing" || req.headers().get("x-method-echo").is_some() {
		let method = req.method().to_string();
		return Ok(
			hyper::Response::builder()
				.status(200)
				.body(Full::<Bytes>::new(Bytes::from(method)))
				.expect("build method-echo"),
		);
	}
	let size = req
		.headers()
		.get("x-body-size")
		.and_then(|v| v.to_str().ok())
		.and_then(|s| s.parse::<usize>().ok())
		.unwrap_or(2);
	let sleep_ms = req
		.headers()
		.get("x-sleep-ms")
		.and_then(|v| v.to_str().ok())
		.and_then(|s| s.parse::<u64>().ok())
		.unwrap_or(0);
	if sleep_ms > 0 {
		tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
	}
	let body = vec![b'x'; size];
	Ok(
		hyper::Response::builder()
			.status(200)
			.body(Full::<Bytes>::new(Bytes::from(body)))
			.expect("build resp"),
	)
}

fn req(url: &str, body_size: usize, sleep_ms: u64) -> HttpFetchRequest {
	let mut headers = vec![("x-body-size".to_string(), body_size.to_string())];
	if sleep_ms > 0 {
		headers.push(("x-sleep-ms".to_string(), sleep_ms.to_string()));
	}
	HttpFetchRequest {
		method: "GET".to_string(),
		url: url.to_string(),
		headers,
		body: Vec::new(),
		timeout_ms: None,
		follow_redirects: None,
		verify_tls: None,
	}
}

static SERVER: tokio::sync::OnceCell<SocketAddr> = tokio::sync::OnceCell::const_new();

async fn upstream_addr() -> SocketAddr {
	*SERVER.get_or_init(spawn_echo_server).await
}

#[tokio::test]
async fn hyper_backend_returns_200_with_body() {
	vane_engine::crypto::install_default_provider();
	let addr = upstream_addr().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let url = format!("http://{addr}/");
	let resp = backend
		.fetch(req(&url, 5, 0), HttpFetchLimits { max_body_bytes: 64, ..HttpFetchLimits::default() })
		.await
		.expect("fetch ok");
	assert_eq!(resp.status, 200);
	assert_eq!(resp.body, vec![b'x'; 5]);
}

#[tokio::test]
async fn hyper_backend_returns_body_too_large_when_response_exceeds_cap() {
	vane_engine::crypto::install_default_provider();
	let addr = upstream_addr().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let url = format!("http://{addr}/");
	let err = backend
		.fetch(req(&url, 1024, 0), HttpFetchLimits { max_body_bytes: 64, ..HttpFetchLimits::default() })
		.await
		.expect_err("must reject oversize body");
	assert!(matches!(err, HttpFetchError::BodyTooLarge), "got {err:?}");
}

#[tokio::test]
async fn hyper_backend_returns_timeout_when_upstream_hangs_past_budget() {
	vane_engine::crypto::install_default_provider();
	let addr = upstream_addr().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let url = format!("http://{addr}/");
	let err = backend
		.fetch(
			req(&url, 1, 500),
			HttpFetchLimits { timeout_ms: Some(50), ..HttpFetchLimits::default() },
		)
		.await
		.expect_err("must time out");
	assert!(matches!(err, HttpFetchError::Timeout), "got {err:?}");
}

#[tokio::test]
async fn hyper_backend_arc_constructor_succeeds() {
	vane_engine::crypto::install_default_provider();
	let backend = HyperHttpFetchBackend::new_arc().expect("arc constructor");
	// Validate it implements the trait through the Arc.
	let _: Arc<dyn HttpFetchBackend> = backend;
}

// redirect-follow
fn redirect_request(from: &str, to: &str, status: u16) -> HttpFetchRequest {
	HttpFetchRequest {
		method: "POST".to_string(),
		url: from.to_string(),
		headers: vec![
			("x-redirect-to".to_string(), to.to_string()),
			("x-redirect-status".to_string(), status.to_string()),
			("x-method-echo".to_string(), "1".to_string()),
		],
		body: b"original-body".to_vec(),
		timeout_ms: None,
		follow_redirects: None,
		verify_tls: None,
	}
}

#[tokio::test]
async fn hyper_backend_follows_302_redirect_with_method_rewrite_to_get() {
	vane_engine::crypto::install_default_provider();
	let addr = upstream_addr().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let from = format!("http://{addr}/start");
	let to = format!("http://{addr}/landing");
	let resp = backend
		.fetch(redirect_request(&from, &to, 302), HttpFetchLimits::default())
		.await
		.expect("fetch ok");
	assert_eq!(resp.status, 200, "final response should be 200");
	// 302 + POST → method rewrites to GET on the redirect hop.
	assert_eq!(String::from_utf8_lossy(&resp.body), "GET");
}

#[tokio::test]
async fn hyper_backend_follows_307_redirect_preserving_method() {
	vane_engine::crypto::install_default_provider();
	let addr = upstream_addr().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let from = format!("http://{addr}/start");
	let to = format!("http://{addr}/landing");
	let resp = backend
		.fetch(redirect_request(&from, &to, 307), HttpFetchLimits::default())
		.await
		.expect("fetch ok");
	assert_eq!(resp.status, 200);
	// 307 preserves the original method.
	assert_eq!(String::from_utf8_lossy(&resp.body), "POST");
}

#[tokio::test]
async fn hyper_backend_returns_redirect_as_is_when_follow_redirects_zero() {
	vane_engine::crypto::install_default_provider();
	let addr = upstream_addr().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let from = format!("http://{addr}/start");
	let to = format!("http://{addr}/landing");
	// `follow_redirects: 0` means "do not follow" — the plugin sees
	// the raw 3xx response and decides what to do.
	let resp = backend
		.fetch(
			redirect_request(&from, &to, 302),
			HttpFetchLimits { follow_redirects: Some(0), ..HttpFetchLimits::default() },
		)
		.await
		.expect("fetch ok");
	assert_eq!(resp.status, 302);
	let location = resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("location"));
	assert!(location.is_some(), "302 response must surface its Location header to the plugin");
}

#[tokio::test]
async fn hyper_backend_resolves_relative_location_against_base_url() {
	vane_engine::crypto::install_default_provider();
	let addr = upstream_addr().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let from = format!("http://{addr}/path/start");
	// Server sends a relative `Location: /landing` — backend must
	// resolve it against the request's authority.
	let resp = backend
		.fetch(redirect_request(&from, "/landing", 302), HttpFetchLimits::default())
		.await
		.expect("fetch ok");
	assert_eq!(resp.status, 200);
}

// insecure-TLS upstream
struct TlsUpstream {
	addr: SocketAddr,
	_cert_keep: NamedTempFile,
}

fn rcgen_self_signed() -> (String, String) {
	let issued = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("rcgen");
	(issued.cert.pem(), issued.signing_key.serialize_pem())
}

async fn spawn_tls_upstream() -> TlsUpstream {
	let (cert_pem, key_pem) = rcgen_self_signed();
	let mut cert_file = NamedTempFile::new().expect("cert tmp");
	cert_file.write_all(cert_pem.as_bytes()).expect("write cert");

	let cert_chain: Vec<rustls_pki_types::CertificateDer<'static>> =
		rustls_pemfile::certs(&mut cert_pem.as_bytes()).map(|c| c.expect("parse cert")).collect();
	let key =
		rustls_pemfile::private_key(&mut key_pem.as_bytes()).expect("parse key").expect("key present");
	let mut server_cfg = rustls::ServerConfig::builder()
		.with_no_client_auth()
		.with_single_cert(cert_chain, key)
		.expect("server config");
	server_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
	let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_cfg));

	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind tls");
	let addr = listener.local_addr().expect("local_addr");
	let accepted = Arc::new(AtomicUsize::new(0));
	let accepted_for_task = Arc::clone(&accepted);
	tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			accepted_for_task.fetch_add(1, Ordering::SeqCst);
			let acceptor = acceptor.clone();
			tokio::spawn(async move {
				let Ok(tls) = acceptor.accept(sock).await else { return };
				let io = TokioIo::new(tls);
				let _ =
					hyper::server::conn::http1::Builder::new().serve_connection(io, service_fn(serve)).await;
			});
		}
	});

	TlsUpstream { addr, _cert_keep: cert_file }
}

#[tokio::test]
async fn hyper_backend_rejects_self_signed_cert_when_allow_insecure_false() {
	vane_engine::crypto::install_default_provider();
	let upstream = spawn_tls_upstream().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let url = format!("https://localhost:{}/", upstream.addr.port());
	let req = HttpFetchRequest {
		method: "GET".to_string(),
		url,
		headers: Vec::new(),
		body: Vec::new(),
		timeout_ms: Some(2_000),
		follow_redirects: None,
		verify_tls: Some(true),
	};
	let err = backend
		.fetch(req, HttpFetchLimits { allow_insecure: false, ..HttpFetchLimits::default() })
		.await
		.expect_err("must reject self-signed cert under verified posture");
	// hyper-util's legacy error doesn't always tag TLS explicitly —
	// accept either TlsError or Internal so long as the call did fail
	// (the contract is "verified-CA path rejects this cert", not the
	// specific error variant).
	assert!(
		matches!(err, HttpFetchError::TlsError(_) | HttpFetchError::Internal(_)),
		"expected verification failure, got {err:?}",
	);
}

#[tokio::test]
async fn hyper_backend_accepts_self_signed_cert_when_allow_insecure_true() {
	vane_engine::crypto::install_default_provider();
	let upstream = spawn_tls_upstream().await;
	let backend = HyperHttpFetchBackend::new().expect("backend");
	let url = format!("https://localhost:{}/", upstream.addr.port());
	let req = HttpFetchRequest {
		method: "GET".to_string(),
		url,
		headers: vec![("x-body-size".to_string(), "3".to_string())],
		body: Vec::new(),
		timeout_ms: Some(2_000),
		follow_redirects: None,
		// `verify_tls: false` is what the host fn would have folded
		// into `limits.allow_insecure: true` — see
		// `vane_wasm::http_fetch_core`.
		verify_tls: Some(false),
	};
	let resp = backend
		.fetch(req, HttpFetchLimits { allow_insecure: true, ..HttpFetchLimits::default() })
		.await
		.expect("must accept self-signed cert under insecure posture");
	assert_eq!(resp.status, 200);
	assert_eq!(resp.body, vec![b'x'; 3]);
}
