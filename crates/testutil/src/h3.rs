//! H3 client helper for engine integration tests.
//!
//! Wraps `quinn::Endpoint::client` + `h3-quinn::Connection` + `h3::client`
//! into a one-call `connect_h3` that yields a `SendRequest` plus the
//! background driver task. Tests use this against a vane H3 listener
//! brought up with a self-signed cert; the helper installs the cert in
//! its rustls root store and pins ALPN to `h3` per RFC 9114.
//!
//! Production code never depends on this module — vane-engine's H3
//! listener side does not use h3-quinn. The client side is allowed to
//! reach for h3-quinn because that's the standard h3 transport when
//! quinn owns its own UDP socket end-to-end (no virtual-socket interpose
//! needed on the client side).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use bytes::Bytes;
use h3_quinn::Connection as H3QuinnConnection;
use quinn::{ClientConfig, Endpoint};
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
