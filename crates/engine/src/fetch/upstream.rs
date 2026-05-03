//! Upstream dial helper for [`super::websocket_upgrade::WebSocketUpgradeFetch`].
//! `HttpProxyFetch` no longer uses this path — it owns a pooled
//! `hyper_util::client::legacy::Client` over `hyper_rustls::HttpsConnector`
//! and selects H1/H2 by ALPN. The WebSocket upgrade path stays on
//! the manual dial because RFC 6455's HTTP/1.1 `Upgrade: websocket`
//! handshake is incompatible with hyper's connection-pool model.
//!
//! Centralising the TLS-config build logic here keeps the rule
//! `args.tls` parser in one place; the same shape feeds both the
//! pooled `HttpProxy` client and the manual WebSocket dial.
//!
//! Trust roots come from the system store via `rustls-native-certs`.
//! `insecure_skip_verify` short-circuits the verifier — testing only;
//! production should leave it `false`. See
//! `spec/architecture/08-tls.md` § _TLS library: rustls only_.

use std::sync::Arc;

use tokio::net::TcpStream;
use vane_core::{AsyncReadWrite, Error, UpstreamReason};

use crate::fetch::client_cache::{RootCaSource, TlsConfigFingerprint, VerifyMode};

/// Built TLS configuration for an upstream dial. Stored on each
/// fetch's `Arc<…>` so the per-call `dial_upstream` only borrows.
///
/// `fingerprint` is the cache key for the daemon-wide
/// `client_cache::ProxyClient` map; `parse_tls_args` populates every
/// field except `alpn_protocols`, which the fetch factory patches in
/// based on the resolved `UpstreamVersion`.
#[derive(Clone)]
pub struct UpstreamTls {
	/// Reusable client config — built once at factory time. Cloning
	/// the `Arc` is cheap; `rustls::ClientConfig` itself is fine to
	/// share across handshakes.
	pub client_config: Arc<rustls::ClientConfig>,
	/// SNI hostname / certificate-verification target. Defaults to
	/// the host portion of the upstream address but operators can
	/// override (e.g. when the upstream is reachable as `127.0.0.1`
	/// but its certificate is issued for `api.internal`).
	pub verify_hostname: String,
	/// Cache key for the daemon-wide `Client` cache. `alpn_protocols`
	/// is left empty here — `parse_tls_args` does not see the fetch's
	/// `version` setting; the factory patches the field once the
	/// version is resolved. See `spec/architecture/08-tls.md`
	/// § _Client cache: fingerprint and reuse_.
	pub fingerprint: TlsConfigFingerprint,
}

/// Dial `upstream` and optionally complete a TLS handshake using the
/// supplied configuration. Returns a boxed [`AsyncReadWrite`] ready
/// for a hyper H1 client handshake. Best-effort `TCP_NODELAY` is set
/// on the underlying socket — small request/response cycles
/// shouldn't sit in Nagle's buffer.
///
/// # Errors
/// - [`UpstreamReason::Unreachable`] if the TCP connect fails.
/// - [`UpstreamReason::TlsHandshake`] if the SNI name parse or the
///   rustls handshake fails. The wrapped source carries the original
///   error message for tracing.
pub async fn dial_upstream(
	upstream: &str,
	tls: Option<&UpstreamTls>,
) -> Result<Box<dyn AsyncReadWrite + Send>, Error> {
	tracing::debug!(?upstream, has_tls = tls.is_some(), "dial_upstream");
	let start = std::time::Instant::now();
	let tcp = TcpStream::connect(upstream).await.map_err(|e| {
		tracing::debug!(?upstream, ?e, "dial_upstream tcp connect failed");
		Error::upstream(UpstreamReason::Unreachable).with_source(e)
	})?;
	metrics::histogram!("vane.upstream.connect.duration_ms", "kind" => "tcp")
		.record(start.elapsed().as_secs_f64() * 1000.0);
	let _ = tcp.set_nodelay(true);

	let Some(tls) = tls else {
		tracing::debug!(?upstream, "dial_upstream cleartext ready");
		return Ok(Box::new(tcp));
	};
	let connector = tokio_rustls::TlsConnector::from(Arc::clone(&tls.client_config));
	let server_name =
		rustls_pki_types::ServerName::try_from(tls.verify_hostname.clone()).map_err(|e| {
			tracing::debug!(?upstream, hostname = %tls.verify_hostname, ?e, "dial_upstream sni parse failed");
			Error::upstream(UpstreamReason::TlsHandshake)
				.with_source(std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))
		})?;
	let tls_stream = connector.connect(server_name, tcp).await.map_err(|e| {
		tracing::debug!(?upstream, hostname = %tls.verify_hostname, ?e, "dial_upstream tls handshake failed");
		Error::upstream(UpstreamReason::TlsHandshake).with_source(e)
	})?;
	tracing::debug!(?upstream, "dial_upstream tls ready");
	Ok(Box::new(tls_stream))
}

/// Build a [`rustls::ClientConfig`] once at fetch factory time.
///
/// `insecure == false` (the default): trust anchors are pulled from
/// the process-wide cached system store
/// ([`crate::tls::native_roots`]). The keychain / NSS store is read
/// once per process; subsequent calls reuse the same `Arc` and never
/// re-touch the OS API.
///
/// `insecure == true`: install [`NoVerify`], a verifier that accepts
/// every certificate. Documented as testing-only in the rule schema;
/// the engine doesn't gate it but operators are responsible for not
/// shipping `insecure_skip_verify: true` to production.
///
/// # Errors
/// String description of any failure to load the system trust store.
/// Returned as `String` because this happens at factory-link time
/// (compile/link errors prefer the lighter-weight shape over a full
/// `Error`).
pub fn build_client_config(insecure: bool) -> Result<Arc<rustls::ClientConfig>, String> {
	if insecure {
		let cfg = rustls::ClientConfig::builder()
			.dangerous()
			.with_custom_certificate_verifier(Arc::new(NoVerify))
			.with_no_client_auth();
		return Ok(Arc::new(cfg));
	}

	let roots = crate::tls::native_roots().map_err(|e| e.message)?;
	let cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
	Ok(Arc::new(cfg))
}

/// `ServerCertVerifier` that accepts any certificate. **Testing only.**
/// Activated by `insecure_skip_verify: true` in the upstream TLS args.
#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
	fn verify_server_cert(
		&self,
		_end_entity: &rustls_pki_types::CertificateDer<'_>,
		_intermediates: &[rustls_pki_types::CertificateDer<'_>],
		_server_name: &rustls_pki_types::ServerName<'_>,
		_ocsp_response: &[u8],
		_now: rustls_pki_types::UnixTime,
	) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
		Ok(rustls::client::danger::ServerCertVerified::assertion())
	}

	fn verify_tls12_signature(
		&self,
		_message: &[u8],
		_cert: &rustls_pki_types::CertificateDer<'_>,
		_dss: &rustls::DigitallySignedStruct,
	) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
		Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
	}

	fn verify_tls13_signature(
		&self,
		_message: &[u8],
		_cert: &rustls_pki_types::CertificateDer<'_>,
		_dss: &rustls::DigitallySignedStruct,
	) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
		Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
	}

	fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
		// Defer to the active crypto provider's supported set so we
		// don't accidentally narrow what a non-skip handshake would
		// accept. The provider is installed once at daemon boot via
		// `vane_engine::crypto::install_default_provider`.
		rustls::crypto::CryptoProvider::get_default()
			.expect("rustls crypto provider installed at boot")
			.signature_verification_algorithms
			.supported_schemes()
	}
}

/// Helper for fetch factories: parse an `args.tls` JSON object into
/// an [`UpstreamTls`]. Returns `Ok(None)` when the field is absent
/// (cleartext upstream). The default `verify_hostname` is the host
/// portion of `upstream` (everything before the trailing `:port`); an
/// explicit `verify_hostname` in the args overrides that.
///
/// # Errors
/// String description of any failure — bad shape, unbuildable client
/// config. Returned as `String` to fit the existing factory error
/// pattern.
pub fn parse_tls_args(
	upstream: &str,
	tls_args: Option<&serde_json::Value>,
) -> Result<Option<UpstreamTls>, String> {
	let Some(tls_args) = tls_args else {
		return Ok(None);
	};
	let verify_hostname =
		tls_args.get("verify_hostname").and_then(serde_json::Value::as_str).map_or_else(
			|| {
				// Default: strip the trailing `:port` if present. `rsplit_once`
				// keeps the whole string when there's no `:` (rare for TCP
				// upstreams but worth the defensive fall-through).
				upstream.rsplit_once(':').map_or(upstream, |(host, _)| host).to_string()
			},
			String::from,
		);
	let insecure =
		tls_args.get("insecure_skip_verify").and_then(serde_json::Value::as_bool).unwrap_or(false);
	let client_config =
		build_client_config(insecure).map_err(|e| format!("build tls client config: {e}"))?;
	// Fingerprint with `alpn_protocols` left empty — the factory
	// patches it once `version` is known. CRL / mTLS slots stay at
	// their post-MVP placeholders.
	let fingerprint = TlsConfigFingerprint {
		root_ca: if insecure { RootCaSource::Skip } else { RootCaSource::System },
		client_cert_hash: None,
		crl_sources: Vec::new(),
		verify_mode: if insecure { VerifyMode::Skip } else { VerifyMode::Full },
		alpn_protocols: Vec::new(),
	};
	Ok(Some(UpstreamTls { client_config, verify_hostname, fingerprint }))
}

#[cfg(test)]
mod tests {
	use super::*;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	use tokio::net::TcpListener;

	#[tokio::test]
	async fn upstream_dial_cleartext_returns_box_async_read_write() {
		// Bind a trivial echo server, dial it without TLS, exchange a few
		// bytes — the dial path that matters here is the `tls.is_none()`
		// branch returning a `Box<dyn AsyncReadWrite>` over the raw
		// TcpStream.
		let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
		let addr = listener.local_addr().expect("local_addr");
		let server = tokio::spawn(async move {
			let (mut sock, _) = listener.accept().await.expect("accept");
			let mut buf = [0u8; 5];
			sock.read_exact(&mut buf).await.expect("read");
			sock.write_all(&buf).await.expect("write");
		});

		let mut conn = dial_upstream(&addr.to_string(), None).await.expect("dial");
		conn.write_all(b"hello").await.expect("write");
		let mut echoed = [0u8; 5];
		conn.read_exact(&mut echoed).await.expect("read echo");
		assert_eq!(&echoed, b"hello");
		server.await.expect("server task");
	}

	#[tokio::test]
	async fn upstream_dial_returns_unreachable_when_port_is_closed() {
		// Ephemeral bind + drop yields an address that's almost certainly
		// closed for the next moment. Dial should surface
		// `UpstreamReason::Unreachable`, not panic.
		let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
		let addr = listener.local_addr().expect("local_addr");
		drop(listener);

		match dial_upstream(&addr.to_string(), None).await {
			Ok(_) => panic!("dial against closed port should fail"),
			Err(e) => assert!(e.to_string().contains("upstream"), "{e}"),
		}
	}

	#[test]
	fn parse_tls_args_returns_none_when_field_absent() {
		assert!(parse_tls_args("api.example.com:443", None).expect("ok").is_none());
	}

	#[test]
	fn parse_tls_args_defaults_verify_hostname_to_host_portion() {
		let parsed = parse_tls_args(
			"api.example.com:443",
			Some(&serde_json::json!({ "insecure_skip_verify": true })),
		)
		.expect("ok")
		.expect("Some");
		assert_eq!(parsed.verify_hostname, "api.example.com");
	}

	#[test]
	fn parse_tls_args_explicit_verify_hostname_overrides_default() {
		let parsed = parse_tls_args(
			"127.0.0.1:9443",
			Some(&serde_json::json!({
				"verify_hostname": "api.internal",
				"insecure_skip_verify": true,
			})),
		)
		.expect("ok")
		.expect("Some");
		assert_eq!(parsed.verify_hostname, "api.internal");
	}
}
