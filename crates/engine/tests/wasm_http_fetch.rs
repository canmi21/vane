//! End-to-end coverage for `HyperHttpFetchBackend`. Spawns a local
//! cleartext echo server and drives requests through the backend the
//! way `vane-wasm`'s `http_fetch_core` would. Exercises:
//!
//! * Happy path 200 with response body collection.
//! * `BodyTooLarge` enforcement when the response exceeds
//!   `limits.max_body_bytes`.
//! * `Timeout` enforcement when the upstream hangs longer than
//!   `limits.timeout_ms`.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
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
