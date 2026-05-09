//! H3 client + server helpers for engine integration tests.
//!
//! Client side: [`connect_h3`] wraps `quinn::Endpoint::client` +
//! `h3-quinn::Connection` + `h3::client` into a one-call helper that
//! yields a `SendRequest` plus the background driver task. Originally
//! used against the vane H3 listener; reused by H3 upstream tests as
//! the test-side H3 client when vane is the proxy.
//!
//! Server side: [`serve_h3`] spawns an h3-quinn-backed server bound
//! to a free UDP port, handles every accepted connection until
//! shutdown, and runs an operator-supplied closure per request. Used
//! by H3 upstream tests as the upstream the vane proxy dials.
//!
//! Production code never depends on this module — vane-engine's H3
//! listener side does not use h3-quinn. The client + server helpers
//! here are allowed to reach for h3-quinn because that's the standard
//! h3 transport when quinn owns its own UDP socket end-to-end (no
//! virtual-socket interpose needed on the test side).

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::{Buf, Bytes};
use h3_quinn::Connection as H3QuinnConnection;
use quinn::{ClientConfig, Endpoint, ServerConfig};
use rustls::pki_types::CertificateDer;

/// Outcome of [`connect_h3`]. Holds the live `SendRequest` plus the
/// `h3::client::Connection` driver future joined into a background task;
/// drop the handle (or call [`Self::shutdown`]) to tear the connection
/// down.
pub struct H3ClientHandle {
	/// Send half — clone-cheap, used to issue requests.
	pub send_request: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
	driver: tokio::task::JoinHandle<()>,
	endpoint: Endpoint,
}

impl H3ClientHandle {
	/// Wait for the connection driver to finish; on cancellation the
	/// driver exits cleanly. Idempotent against repeated drops.
	pub async fn shutdown(self) {
		drop(self.send_request);
		// Endpoint::close shuts down the underlying quinn endpoint; the
		// driver future then resolves. No deadline argument — the test
		// harness wraps this in its own tokio::time::timeout if needed.
		self.endpoint.close(0u32.into(), b"test done");
		let _ = self.driver.await;
	}
}

/// Build a client `quinn::Endpoint` bound to an ephemeral UDP port,
/// configured with `cert_pem` as the only trusted root and ALPN
/// `[b"h3"]`. The caller is expected to have already installed a
/// rustls crypto provider (engine tests call
/// `vane_engine::crypto::install_default_provider` in their setup).
///
/// # Errors
///
/// Returns a stringly error for cert parse failure, rustls config build
/// failure, UDP bind failure, or quinn endpoint construction failure.
fn build_client_endpoint(cert_pem: &str) -> Result<Endpoint, String> {
	let mut roots = rustls::RootCertStore::empty();
	for cert in rustls_pemfile::certs(&mut cert_pem.as_bytes()) {
		let cert: CertificateDer<'_> = cert.map_err(|e| format!("parse pem cert: {e}"))?;
		roots.add(cert).map_err(|e| format!("add cert to root store: {e}"))?;
	}
	let mut tls = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
	tls.alpn_protocols = vec![b"h3".to_vec()];
	let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
		.map_err(|e| format!("quinn QuicClientConfig from rustls: {e}"))?;
	let client_cfg = ClientConfig::new(Arc::new(quic_crypto));

	// Bind ephemeral on loopback v4 — engine tests bind their listeners on
	// loopback v4 too, so client and server share the family.
	let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
	let mut endpoint =
		Endpoint::client(bind_addr).map_err(|e| format!("client endpoint bind: {e}"))?;
	endpoint.set_default_client_config(client_cfg);
	Ok(endpoint)
}

/// Open an H3 connection against `server_addr`, presenting `sni` as the
/// TLS server name and `cert_pem` as the only trusted root.
///
/// Returns a [`H3ClientHandle`] whose `send_request` is ready to issue
/// HTTP/3 requests; the handshake is complete by the time this resolves.
///
/// # Errors
///
/// Returns a stringly error for any setup failure: rustls config build,
/// quinn endpoint bind, QUIC handshake, h3 negotiation. Tests should
/// `expect()` on this — a failure means the H3 listener under test
/// didn't come up correctly.
pub async fn connect_h3(
	server_addr: SocketAddr,
	cert_pem: &str,
	sni: &str,
) -> Result<H3ClientHandle, String> {
	let endpoint = build_client_endpoint(cert_pem)?;
	let connecting =
		endpoint.connect(server_addr, sni).map_err(|e| format!("quinn connect call: {e}"))?;
	let quic_conn = connecting.await.map_err(|e| format!("quinn handshake: {e}"))?;
	let h3_quic = H3QuinnConnection::new(quic_conn);
	let (mut driver, send_request) =
		h3::client::new(h3_quic).await.map_err(|e| format!("h3 client setup: {e}"))?;
	let driver = tokio::spawn(async move {
		// `wait_idle` resolves with the terminal connection error once
		// every stream has finished and the peer (or local) closed the
		// connection. Tests don't inspect the error — they just need
		// the driver future to be polled while the connection lives.
		let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
	});
	Ok(H3ClientHandle { send_request, driver, endpoint })
}

// Server side

/// Outcome of [`serve_h3`]. Holds the bound `SocketAddr`, the
/// connection counter (used by tests asserting pool sharing), and
/// the accept-loop task handle. Drop is best-effort cleanup; tests
/// can also call [`Self::shutdown`] for synchronous teardown.
pub struct H3ServerHandle {
	pub addr: SocketAddr,
	accept_count: Arc<AtomicUsize>,
	endpoint: Endpoint,
	join: Option<tokio::task::JoinHandle<()>>,
}

impl H3ServerHandle {
	/// Number of QUIC connections accepted by this server's accept
	/// loop since boot. Tests assert pool sharing by issuing N
	/// requests to the same `vane` proxy fingerprint and checking
	/// that this counter remains 1.
	#[must_use]
	pub fn accept_count(&self) -> usize {
		self.accept_count.load(Ordering::SeqCst)
	}

	/// Shut the server down. The endpoint is closed (terminating any
	/// in-flight connections) and the accept loop is awaited.
	pub async fn shutdown(mut self) {
		self.endpoint.close(0u32.into(), b"test done");
		if let Some(j) = self.join.take() {
			let _ = j.await;
		}
	}
}

impl Drop for H3ServerHandle {
	fn drop(&mut self) {
		// Best-effort: closing the endpoint signals the accept loop to
		// exit; aborting the join handle prevents the task from
		// outliving the handle when tests forget to call shutdown.
		self.endpoint.close(0u32.into(), b"handle dropped");
		if let Some(j) = self.join.take() {
			j.abort();
		}
	}
}

/// Spawn an h3-quinn-backed server bound to an ephemeral UDP port
/// on loopback v4. The server presents `cert_pem` + `key_pem` to
/// every TLS handshake (operator generates them via `rcgen`), accepts
/// connections until shutdown, and runs `handler` for each accepted
/// request stream.
///
/// `handler` receives the request head (`http::Request<()>`) and the
/// fully-buffered request body, and returns
/// `(StatusCode, response body bytes)`. Buffering the body inside the
/// helper keeps the test-side handler signature simple — production
/// upstream paths stream, but a test fixture optimizing for clarity
/// over scale is the right trade.
///
/// Returns once the endpoint is bound; the accept loop continues
/// in the background until [`H3ServerHandle::shutdown`] is called or
/// the handle is dropped.
///
/// # Errors
///
/// Returns a stringly error for any setup failure (cert parse, rustls
/// config, quinn server config, UDP bind).
///
/// # Panics
///
/// Panics if the synthetic `http::Response` builder rejects the
/// status code returned by `handler` — only the test author can
/// trigger this, by returning a value outside `http::StatusCode`'s
/// valid range, which the type itself already prevents.
#[allow(
	clippy::unused_async,
	reason = "intentional API shape: mirrors connect_h3 await form and signals the tokio runtime requirement of the inner spawn"
)]
pub async fn serve_h3<F, Fut>(
	cert_pem: &str,
	key_pem: &str,
	handler: F,
) -> Result<H3ServerHandle, String>
where
	F: Fn(http::Request<()>, Vec<u8>) -> Fut + Send + Sync + Clone + 'static,
	Fut: Future<Output = (http::StatusCode, Vec<u8>)> + Send + 'static,
{
	let endpoint = build_server_endpoint(cert_pem, key_pem)?;
	let addr = endpoint.local_addr().map_err(|e| format!("server local_addr: {e}"))?;
	let accept_count = Arc::new(AtomicUsize::new(0));
	let counter = Arc::clone(&accept_count);
	let endpoint_for_loop = endpoint.clone();
	let join = tokio::spawn(async move {
		while let Some(incoming) = endpoint_for_loop.accept().await {
			counter.fetch_add(1, Ordering::SeqCst);
			let handler = handler.clone();
			tokio::spawn(async move {
				let Ok(connecting) = incoming.accept() else { return };
				let Ok(quic_conn) = connecting.await else { return };
				let h3_quic = H3QuinnConnection::new(quic_conn);
				let Ok(mut h3_conn) = h3::server::Connection::new(h3_quic).await else { return };
				loop {
					let Ok(Some(resolver)) = h3_conn.accept().await else { return };
					let handler = handler.clone();
					tokio::spawn(async move {
						let Ok((req, mut stream)) = resolver.resolve_request().await else { return };
						// Buffer the request body — tests are small.
						let mut body_buf = Vec::new();
						loop {
							match stream.recv_data().await {
								Ok(Some(mut chunk)) => {
									let remaining = chunk.remaining();
									let bytes = chunk.copy_to_bytes(remaining);
									body_buf.extend_from_slice(&bytes);
								}
								Ok(None) => break,
								Err(_) => return,
							}
						}
						let (status, resp_body) = handler(req, body_buf).await;
						let resp = http::Response::builder().status(status).body(()).expect("build resp");
						if stream.send_response(resp).await.is_err() {
							return;
						}
						if !resp_body.is_empty() && stream.send_data(Bytes::from(resp_body)).await.is_err() {
							return;
						}
						let _ = stream.finish().await;
					});
				}
			});
		}
	});
	Ok(H3ServerHandle { addr, accept_count, endpoint, join: Some(join) })
}

/// Build the `quinn::Endpoint` for an h3 test server bound to
/// loopback v4 ephemeral. ALPN is pinned to `[b"h3"]`; the cert +
/// key feed a single-cert rustls server config (no SNI multi-cert
/// resolution needed for tests).
fn build_server_endpoint(cert_pem: &str, key_pem: &str) -> Result<Endpoint, String> {
	let cert_chain = rustls_pemfile::certs(&mut cert_pem.as_bytes())
		.collect::<Result<Vec<CertificateDer<'static>>, _>>()
		.map_err(|e| format!("parse pem cert chain: {e}"))?;
	let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
		.map_err(|e| format!("parse pem private key: {e}"))?
		.ok_or_else(|| "no private key found in pem".to_string())?;
	let mut tls = rustls::ServerConfig::builder()
		.with_no_client_auth()
		.with_single_cert(cert_chain, key)
		.map_err(|e| format!("rustls server config: {e}"))?;
	tls.alpn_protocols = vec![b"h3".to_vec()];
	let quic_crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
		.map_err(|e| format!("quinn QuicServerConfig from rustls: {e}"))?;
	let server_cfg = ServerConfig::with_crypto(Arc::new(quic_crypto));

	let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
	let endpoint =
		Endpoint::server(server_cfg, bind_addr).map_err(|e| format!("server endpoint bind: {e}"))?;
	Ok(endpoint)
}
