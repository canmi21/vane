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
//! `spec/crates/engine-tls.md` § _Library policy_.

use std::path::PathBuf;
use std::sync::Arc;

use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;
use sha2::Digest as _;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use vane_core::{AsyncReadWrite, Error, TimeoutKind, UpstreamReason};

use crate::fetch::client_cache::{CrlSource, RootCaSource, TlsConfigFingerprint, VerifyMode};
use crate::tls::crl_cache::{CrlFetchFailure, CrlSourceId};

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
	/// version is resolved. See `spec/crates/engine-tls.md`
	/// § _Client cache_.
	pub fingerprint: TlsConfigFingerprint,
	/// Resolved `args.tls.crls` source list — kept on the value so the
	/// daemon can register them with the shared `CrlCache` at link time.
	/// The bytes themselves live in the cache, not here. Empty when no
	/// CRL sources are configured (the common case).
	pub crls: Vec<(CrlSourceId, CrlFetchFailure)>,
	/// Optional client certificate for upstream mTLS. Spec § _Upstream-side TLS_ — `Arc<CertifiedKey>` is the daemon-wide sharing
	/// primitive (rustls' `CertifiedKey` is intentionally not `Clone`).
	/// `None` is the common one-way TLS path.
	pub client_cert: Option<Arc<CertifiedKey>>,
}

/// Default per-dial timeout used by [`dial_upstream`] when the caller
/// has no operator-supplied budget. 10 s matches what the H1 client
/// pool uses for its own connect leg.
pub const DEFAULT_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Dial `upstream` and optionally complete a TLS handshake using the
/// supplied configuration. Returns a boxed [`AsyncReadWrite`] ready
/// for a hyper H1 client handshake. Best-effort `TCP_NODELAY` is set
/// on the underlying socket — small request/response cycles
/// shouldn't sit in Nagle's buffer.
///
/// `cancel` propagates the per-connection cancel token: when the
/// listener-level `force_cancel` fires (or the executor itself drops
/// the fetch future for any reason) the in-flight TCP / TLS handshake
/// aborts immediately, surfacing as
/// [`UpstreamReason::Unreachable`]. Without this select the dial
/// could happily complete after the rest of the connection has been
/// torn down.
///
/// `connect_timeout` caps the entire `(connect + tls)` window —
/// individual stages don't get their own budgets because the caller's
/// view is "did the upstream answer in time?". On expiry the error
/// surfaces as [`TimeoutKind::Connect`].
///
/// # Errors
/// - [`UpstreamReason::Unreachable`] if the TCP connect fails or the
///   cancel token fires mid-dial.
/// - [`UpstreamReason::TlsHandshake`] if the SNI name parse or the
///   rustls handshake fails. The wrapped source carries the original
///   error message for tracing.
/// - [`TimeoutKind::Connect`] if `connect_timeout` elapses.
pub async fn dial_upstream(
	upstream: &str,
	tls: Option<&UpstreamTls>,
	cancel: &CancellationToken,
	connect_timeout: Duration,
) -> Result<Box<dyn AsyncReadWrite + Send>, Error> {
	let dial = dial_upstream_inner(upstream, tls, cancel);
	if let Ok(result) = tokio::time::timeout(connect_timeout, dial).await {
		result
	} else {
		tracing::debug!(?upstream, ?connect_timeout, "dial_upstream timed out");
		Err(Error::timeout(TimeoutKind::Connect))
	}
}

async fn dial_upstream_inner(
	upstream: &str,
	tls: Option<&UpstreamTls>,
	cancel: &CancellationToken,
) -> Result<Box<dyn AsyncReadWrite + Send>, Error> {
	tracing::debug!(?upstream, has_tls = tls.is_some(), "dial_upstream");
	let start = std::time::Instant::now();
	let tcp = tokio::select! {
		biased;
		() = cancel.cancelled() => {
			tracing::debug!(?upstream, "dial_upstream cancelled during tcp connect");
			return Err(Error::canceled());
		}
		res = TcpStream::connect(upstream) => res.map_err(|e| {
			tracing::debug!(?upstream, ?e, "dial_upstream tcp connect failed");
			Error::upstream(UpstreamReason::Unreachable).with_source(e)
		})?,
	};
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
	let tls_stream = tokio::select! {
		biased;
		() = cancel.cancelled() => {
			tracing::debug!(?upstream, "dial_upstream cancelled during tls handshake");
			return Err(Error::canceled());
		}
		res = connector.connect(server_name, tcp) => res.map_err(|e| {
			tracing::debug!(?upstream, hostname = %tls.verify_hostname, ?e, "dial_upstream tls handshake failed");
			Error::upstream(UpstreamReason::TlsHandshake).with_source(e)
		})?,
	};
	tracing::debug!(?upstream, "dial_upstream tls ready");
	Ok(Box::new(tls_stream))
}

/// Build a [`rustls::ClientConfig`] once at fetch factory time.
///
/// `insecure == false` (the default): trust anchors are pulled from
/// the process-wide cached system store
/// (`crate::tls::native_roots`). The keychain / NSS store is read
/// once per process; subsequent calls reuse the same `Arc` and never
/// re-touch the OS API.
///
/// `insecure == true`: install a `NoVerify` verifier that accepts
/// every certificate. Documented as testing-only in the rule schema;
/// the engine doesn't gate it but operators are responsible for not
/// shipping `insecure_skip_verify: true` to production.
///
/// CRLs are not consulted on this path; callers that need revocation
/// checking go through [`build_client_config_with_crls`] which installs
/// a [`crate::tls::RefreshableServerCertVerifier`] over the same trust
/// roots.
///
/// # Errors
/// String description of any failure to load the system trust store.
/// Returned as `String` because this happens at factory-link time
/// (compile/link errors prefer the lighter-weight shape over a full
/// `Error`).
pub fn build_client_config(insecure: bool) -> Result<Arc<rustls::ClientConfig>, String> {
	build_client_config_with_crls(insecure, None, &[], None)
}

/// Like [`build_client_config`] but installs a refreshable
/// `ServerCertVerifier` when `crls` is non-empty. The CRL bytes
/// themselves come from `crl_cache` per handshake.
///
/// `insecure == true` short-circuits (CRLs are meaningless against
/// `NoVerify`); the call is silently equivalent to
/// `build_client_config(true)`.
///
/// `cleartext` upstreams never reach this path — `parse_tls_args`
/// returns `Ok(None)` and the dial path skips the rustls connector.
///
/// # Errors
/// String description of any failure to load the system trust store
/// or build the wrapper verifier.
pub fn build_client_config_with_crls(
	insecure: bool,
	crl_cache: Option<&Arc<crate::tls::CrlCache>>,
	crls: &[(CrlSourceId, CrlFetchFailure)],
	client_cert: Option<&Arc<CertifiedKey>>,
) -> Result<Arc<rustls::ClientConfig>, String> {
	if insecure {
		let builder = rustls::ClientConfig::builder()
			.dangerous()
			.with_custom_certificate_verifier(Arc::new(NoVerify));
		return Ok(Arc::new(finish_client_auth(builder, client_cert)));
	}

	let roots = crate::tls::native_roots().map_err(|e| e.message)?;
	if crls.is_empty() {
		let builder = rustls::ClientConfig::builder().with_root_certificates(roots);
		return Ok(Arc::new(finish_client_auth(builder, client_cert)));
	}

	let cache = crl_cache
		.cloned()
		.ok_or_else(|| "upstream tls.crls configured but daemon CrlCache not provided".to_string())?;
	let sources: Vec<CrlSourceId> = crls.iter().map(|(id, _)| id.clone()).collect();
	let verifier = crate::tls::RefreshableServerCertVerifier::new(cache, sources, roots);
	let builder =
		rustls::ClientConfig::builder().dangerous().with_custom_certificate_verifier(verifier);
	Ok(Arc::new(finish_client_auth(builder, client_cert)))
}

fn finish_client_auth(
	builder: rustls::ConfigBuilder<rustls::ClientConfig, rustls::client::WantsClientCert>,
	client_cert: Option<&Arc<CertifiedKey>>,
) -> rustls::ClientConfig {
	match client_cert {
		Some(ck) => {
			let resolver: Arc<dyn rustls::client::ResolvesClientCert> =
				Arc::new(rustls::sign::SingleCertAndKey::from(Arc::clone(ck)));
			builder.with_client_cert_resolver(resolver)
		}
		None => builder.with_no_client_auth(),
	}
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

/// Read the process-wide `VANE_ALLOW_INSECURE_UPSTREAM` env var as a
/// truthy boolean. The check happens at config-parse time per-call so
/// a hot reload that flips the env var (typically via SIGHUP-style
/// supervisor restart) reflects without bouncing the daemon — but the
/// usual lifecycle is "set once at boot, never changes again".
///
/// Truthy values: `1`, `true`, `yes`, `on`, case-insensitive.
fn allow_insecure_upstream_env() -> bool {
	matches!(
		std::env::var("VANE_ALLOW_INSECURE_UPSTREAM").ok().map(|s| s.to_ascii_lowercase()).as_deref(),
		Some("1" | "true" | "yes" | "on"),
	)
}

/// Normalise a hostname before handing it to `rustls`. The pipeline
/// is: IDNA → ASCII (so unicode IDNs match the cert's punycode SANs),
/// ASCII-lowercase (rustls compares case-insensitively, but matching
/// the cert verbatim avoids subtle bugs in middleware that hashes the
/// raw string), and strip a single trailing FQDN dot (`example.com.`
/// is the same host as `example.com` for cert matching but rustls's
/// `ServerName` parser rejects the trailing dot).
///
/// Failures here mean an operator wrote a hostname that `rustls`
/// would later reject at handshake time; surfacing the error at
/// config-parse time turns "first connection fails mysteriously"
/// into a deterministic compile error.
///
/// # Errors
/// String description for empty input, non-ASCII bytes that IDNA
/// can't translate, or addresses with embedded spaces / control
/// chars.
fn normalize_verify_hostname(host: &str) -> Result<String, String> {
	if host.is_empty() {
		return Err("hostname is empty".into());
	}
	// Strip a single trailing FQDN dot. `rstrip_*` would chew through
	// `a..` which is invalid for other reasons; one dot is the only
	// legal case we need to tolerate.
	let trimmed = host.strip_suffix('.').unwrap_or(host);
	let ascii = idna::domain_to_ascii(trimmed)
		.map_err(|e| format!("idna domain_to_ascii rejected hostname: {e}"))?;
	let mut lowered = ascii;
	lowered.make_ascii_lowercase();
	Ok(lowered)
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
	crl_cache: Option<&Arc<crate::tls::CrlCache>>,
) -> Result<Option<UpstreamTls>, String> {
	let Some(tls_args) = tls_args else {
		return Ok(None);
	};
	let raw_hostname =
		tls_args.get("verify_hostname").and_then(serde_json::Value::as_str).map_or_else(
			|| {
				// Default: strip the trailing `:port` if present. `rsplit_once`
				// keeps the whole string when there's no `:` (rare for TCP
				// upstreams but worth the defensive fall-through).
				upstream.rsplit_once(':').map_or(upstream, |(host, _)| host).to_string()
			},
			String::from,
		);
	let verify_hostname = normalize_verify_hostname(&raw_hostname)
		.map_err(|e| format!("verify_hostname {raw_hostname:?}: {e}"))?;
	let insecure =
		tls_args.get("insecure_skip_verify").and_then(serde_json::Value::as_bool).unwrap_or(false);
	if insecure && !allow_insecure_upstream_env() {
		return Err(
			"tls.insecure_skip_verify=true requires VANE_ALLOW_INSECURE_UPSTREAM=1 in the daemon environment; \
			 refusing to disable upstream cert verification by config alone"
				.to_owned(),
		);
	}
	let crls = parse_crls(tls_args.get("crls"))?;
	let client_cert = parse_client_cert(tls_args.get("client_cert"))?;
	let client_config =
		build_client_config_with_crls(insecure, crl_cache, &crls, client_cert.as_ref())
			.map_err(|e| format!("build tls client config: {e}"))?;
	// Fingerprint with `alpn_protocols` left empty — the factory
	// patches it once `version` is known. CRL slots are populated from
	// the parsed source list; client_cert_hash is SHA-256 of the leaf
	// cert DER when upstream mTLS is on.
	let crl_sources: Vec<CrlSource> = crls
		.iter()
		.map(|(id, _)| match id {
			CrlSourceId::File(p) => CrlSource::File(p.clone()),
			CrlSourceId::Url(u) => CrlSource::Url(u.clone()),
		})
		.collect();
	let fingerprint = TlsConfigFingerprint {
		root_ca: if insecure { RootCaSource::Skip } else { RootCaSource::System },
		client_cert_hash: client_cert.as_ref().map(|ck| client_cert_fingerprint(ck)),
		crl_sources,
		verify_mode: if insecure { VerifyMode::Skip } else { VerifyMode::Full },
		alpn_protocols: Vec::new(),
	};
	Ok(Some(UpstreamTls { client_config, verify_hostname, fingerprint, crls, client_cert }))
}

/// SHA-256 of the leaf certificate DER. Used as the
/// `client_cert_hash` slot on `TlsConfigFingerprint` so two rules
/// loading the same upstream cert (and key) share one
/// `Arc<ClientConfig>`; rotating the cert produces a new `Arc` and
/// therefore a new pool entry.
fn client_cert_fingerprint(ck: &CertifiedKey) -> [u8; 32] {
	let mut hasher = sha2::Sha256::new();
	if let Some(leaf) = ck.cert.first() {
		hasher.update(leaf.as_ref());
	}
	hasher.finalize().into()
}

/// Parse `args.tls.client_cert` into an [`Arc<CertifiedKey>`]. Both
/// `cert_path` and `key_path` are required when the object is
/// present; either being absent is a rule-level compile error
/// (`spec/crates/engine-tls.md` § _Upstream-side TLS_).
fn parse_client_cert(
	value: Option<&serde_json::Value>,
) -> Result<Option<Arc<CertifiedKey>>, String> {
	let Some(obj) = value else {
		return Ok(None);
	};
	let cert_path = obj
		.get("cert_path")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| "tls.client_cert.cert_path missing".to_string())?;
	let key_path = obj
		.get("key_path")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| "tls.client_cert.key_path missing".to_string())?;
	load_certified_key(&PathBuf::from(cert_path), &PathBuf::from(key_path))
		.map(Some)
		.map_err(|e| format!("tls.client_cert: {e}"))
}

fn load_certified_key(
	cert_path: &std::path::Path,
	key_path: &std::path::Path,
) -> Result<Arc<CertifiedKey>, String> {
	let cert_bytes =
		std::fs::read(cert_path).map_err(|e| format!("read cert_path {}: {e}", cert_path.display()))?;
	let key_bytes =
		std::fs::read(key_path).map_err(|e| format!("read key_path {}: {e}", key_path.display()))?;
	let mut cert_reader = std::io::BufReader::new(cert_bytes.as_slice());
	let mut chain: Vec<CertificateDer<'static>> = Vec::new();
	for der in rustls_pemfile::certs(&mut cert_reader) {
		chain.push(der.map_err(|e| format!("parse cert_path: {e}"))?);
	}
	if chain.is_empty() {
		return Err(format!("cert_path {} has no certs", cert_path.display()));
	}
	let key_der: PrivateKeyDer<'static> =
		rustls_pemfile::private_key(&mut std::io::BufReader::new(key_bytes.as_slice()))
			.map_err(|e| format!("parse key_path: {e}"))?
			.ok_or_else(|| format!("key_path {} has no private key", key_path.display()))?;
	let provider = rustls::crypto::CryptoProvider::get_default()
		.ok_or_else(|| "rustls crypto provider not installed".to_string())?;
	let ck = CertifiedKey::from_der(chain, key_der, provider)
		.map_err(|e| format!("CertifiedKey::from_der: {e}"))?;
	Ok(Arc::new(ck))
}

fn parse_crls(
	value: Option<&serde_json::Value>,
) -> Result<Vec<(CrlSourceId, CrlFetchFailure)>, String> {
	let Some(arr) = value else {
		return Ok(Vec::new());
	};
	let entries = arr.as_array().ok_or_else(|| "args.tls.crls must be an array".to_string())?;
	entries
		.iter()
		.enumerate()
		.map(|(idx, entry)| {
			let cfg: vane_core::rule::CrlSourceConfig =
				serde_json::from_value(entry.clone()).map_err(|e| format!("args.tls.crls[{idx}]: {e}"))?;
			Ok(crate::tls::client_trust::crl_source_from_config(&cfg))
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	use tokio::net::TcpListener;

	#[test]
	fn normalize_verify_hostname_lowercases_and_strips_trailing_dot() {
		assert_eq!(normalize_verify_hostname("Example.COM.").unwrap(), "example.com");
		assert_eq!(normalize_verify_hostname("api.example.com").unwrap(), "api.example.com");
	}

	#[test]
	fn normalize_verify_hostname_idna_translates_unicode_to_punycode() {
		// `bücher.example` is the canonical IDN test vector; IDNA
		// maps it to `xn--bcher-kva.example`.
		let got = normalize_verify_hostname("Bücher.Example").expect("idna");
		assert_eq!(got, "xn--bcher-kva.example");
	}

	#[test]
	fn normalize_verify_hostname_rejects_empty() {
		assert!(normalize_verify_hostname("").is_err());
	}

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

		let mut conn = dial_upstream(
			&addr.to_string(),
			None,
			&tokio_util::sync::CancellationToken::new(),
			DEFAULT_DIAL_TIMEOUT,
		)
		.await
		.expect("dial");
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

		match dial_upstream(
			&addr.to_string(),
			None,
			&tokio_util::sync::CancellationToken::new(),
			DEFAULT_DIAL_TIMEOUT,
		)
		.await
		{
			Ok(_) => panic!("dial against closed port should fail"),
			Err(e) => assert!(e.to_string().contains("upstream"), "{e}"),
		}
	}

	#[test]
	fn parse_tls_args_returns_none_when_field_absent() {
		assert!(parse_tls_args("api.example.com:443", None, None).expect("ok").is_none());
	}

	#[test]
	fn parse_tls_args_defaults_verify_hostname_to_host_portion() {
		// `insecure_skip_verify` would otherwise be gated by the env
		// master switch; this test only exercises the hostname
		// defaulting path, so explicitly leave the flag unset.
		crate::crypto::install_default_provider();
		let parsed = parse_tls_args("api.example.com:443", Some(&serde_json::json!({})), None)
			.expect("ok")
			.expect("Some");
		assert_eq!(parsed.verify_hostname, "api.example.com");
	}

	#[test]
	fn parse_tls_args_explicit_verify_hostname_overrides_default() {
		crate::crypto::install_default_provider();
		let parsed = parse_tls_args(
			"127.0.0.1:9443",
			Some(&serde_json::json!({ "verify_hostname": "api.internal" })),
			None,
		)
		.expect("ok")
		.expect("Some");
		assert_eq!(parsed.verify_hostname, "api.internal");
	}

	#[test]
	fn parse_tls_args_rejects_insecure_skip_verify_without_env_opt_in() {
		// The master-switch contract: per-upstream `insecure_skip_verify`
		// is rejected unless VANE_ALLOW_INSECURE_UPSTREAM is set in
		// the daemon env. The unit-test environment never sets it.
		crate::crypto::install_default_provider();
		let Err(err) = parse_tls_args(
			"api.example.com:443",
			Some(&serde_json::json!({ "insecure_skip_verify": true })),
			None,
		) else {
			panic!("must reject without env opt-in")
		};
		assert!(err.contains("VANE_ALLOW_INSECURE_UPSTREAM"), "error names env var: {err}");
	}
}
