use std::io;
use std::sync::Arc;
use std::time::Duration;

use rustls::crypto::ring::sign::any_supported_type;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ProtocolVersion, ServerConfig};
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

use super::cert::CertStore;

#[derive(Debug, thiserror::Error)]
pub enum TlsAcceptError {
	#[error("TLS handshake timed out after {timeout_secs}s")]
	HandshakeTimeout { timeout_secs: u64 },
	#[error("TLS handshake failed")]
	HandshakeFailed(#[source] io::Error),
	#[error("no certificate configured in cert store")]
	NoCertificateConfigured,
}

#[derive(Debug, Clone)]
pub struct TlsInfo {
	pub sni: Option<String>,
	pub alpn: Option<String>,
	/// Human-readable TLS version, e.g. "TLSv1.3"
	pub tls_version: Option<String>,
}

#[derive(Debug)]
struct SniCertResolver {
	store: Arc<CertStore>,
}

impl ResolvesServerCert for SniCertResolver {
	fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
		let name = client_hello.server_name().unwrap_or("default");
		let loaded = self.store.get(name)?;
		let signing_key = any_supported_type(loaded.key()).ok()?;
		let certified = CertifiedKey::new(loaded.certs().to_vec(), signing_key);
		Some(Arc::new(certified))
	}
}

/// Build a rustls [`ServerConfig`] with SNI-based certificate resolution.
///
/// Returns [`TlsAcceptError::NoCertificateConfigured`] when the store is empty.
pub fn build_server_config(
	store: Arc<CertStore>,
	alpn: &[String],
) -> Result<Arc<ServerConfig>, TlsAcceptError> {
	if store.is_empty() {
		return Err(TlsAcceptError::NoCertificateConfigured);
	}

	let mut config =
		ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
			.with_safe_default_protocol_versions()
			.map_err(|e| TlsAcceptError::HandshakeFailed(io::Error::other(e)))?
			.with_no_client_auth()
			.with_cert_resolver(Arc::new(SniCertResolver { store }));

	config.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();

	Ok(Arc::new(config))
}

/// Perform a TLS handshake on an accepted TCP connection.
///
/// Returns the TLS stream and extracted handshake metadata ([`TlsInfo`]).
pub async fn accept_tls(
	stream: TcpStream,
	config: &Arc<ServerConfig>,
	timeout: Duration,
) -> Result<(tokio_rustls::server::TlsStream<TcpStream>, TlsInfo), TlsAcceptError> {
	debug!("starting TLS handshake");

	let acceptor = TlsAcceptor::from(config.clone());

	let tls_stream = tokio::time::timeout(timeout, acceptor.accept(stream))
		.await
		.map_err(|_| TlsAcceptError::HandshakeTimeout { timeout_secs: timeout.as_secs() })?
		.map_err(|e| {
			warn!(error = %e, "TLS handshake failed");
			TlsAcceptError::HandshakeFailed(e)
		})?;

	let (_, server_conn) = tls_stream.get_ref();

	let sni = server_conn.server_name().map(String::from);
	let alpn = server_conn.alpn_protocol().map(|p| String::from_utf8_lossy(p).into_owned());
	let tls_version = server_conn.protocol_version().map(format_tls_version);

	let tls_info = TlsInfo { sni, alpn, tls_version };

	info!(
		sni = tls_info.sni.as_deref().unwrap_or("-"),
		alpn = tls_info.alpn.as_deref().unwrap_or("-"),
		version = tls_info.tls_version.as_deref().unwrap_or("-"),
		"tls.handshake_complete"
	);

	Ok((tls_stream, tls_info))
}

fn format_tls_version(version: ProtocolVersion) -> String {
	if version == ProtocolVersion::TLSv1_3 {
		"TLSv1.3".to_owned()
	} else if version == ProtocolVersion::TLSv1_2 {
		"TLSv1.2".to_owned()
	} else {
		format!("TLS({version:?})")
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
	use super::super::cert::parse_pem;
	use super::*;
	use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
	use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
	use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
	use tokio::net::TcpListener;
	use tokio_rustls::TlsConnector;

	fn generate_self_signed(domains: Vec<String>) -> (Vec<u8>, Vec<u8>) {
		let cert = rcgen::generate_simple_self_signed(domains).unwrap();
		let cert_pem = cert.cert.pem().into_bytes();
		let key_pem = cert.signing_key.serialize_pem().into_bytes();
		(cert_pem, key_pem)
	}

	#[derive(Debug)]
	struct NoVerify;

	impl ServerCertVerifier for NoVerify {
		fn verify_server_cert(
			&self,
			_end_entity: &CertificateDer<'_>,
			_intermediates: &[CertificateDer<'_>],
			_server_name: &ServerName<'_>,
			_ocsp_response: &[u8],
			_now: UnixTime,
		) -> Result<ServerCertVerified, rustls::Error> {
			Ok(ServerCertVerified::assertion())
		}

		fn verify_tls12_signature(
			&self,
			_message: &[u8],
			_cert: &CertificateDer<'_>,
			_dss: &DigitallySignedStruct,
		) -> Result<HandshakeSignatureValid, rustls::Error> {
			Ok(HandshakeSignatureValid::assertion())
		}

		fn verify_tls13_signature(
			&self,
			_message: &[u8],
			_cert: &CertificateDer<'_>,
			_dss: &DigitallySignedStruct,
		) -> Result<HandshakeSignatureValid, rustls::Error> {
			Ok(HandshakeSignatureValid::assertion())
		}

		fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
			rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
		}
	}

	fn build_test_client_config(alpn: &[&str]) -> Arc<ClientConfig> {
		let provider = Arc::new(rustls::crypto::ring::default_provider());
		let mut config = ClientConfig::builder_with_provider(provider)
			.with_safe_default_protocol_versions()
			.unwrap()
			.dangerous()
			.with_custom_certificate_verifier(Arc::new(NoVerify))
			.with_no_client_auth();
		config.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
		Arc::new(config)
	}

	#[tokio::test]
	async fn handshake_success() {
		let (cert_pem, key_pem) = generate_self_signed(vec!["localhost".to_owned()]);
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();

		let mut store = CertStore::new();
		store.insert("default", loaded);

		let server_config = build_server_config(Arc::new(store), &[]).unwrap();

		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let sc = server_config.clone();
		let server = tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			accept_tls(stream, &sc, Duration::from_secs(5)).await.unwrap()
		});

		let client_config = build_test_client_config(&[]);
		let connector = TlsConnector::from(client_config);
		let tcp = TcpStream::connect(addr).await.unwrap();
		let server_name = ServerName::try_from("localhost").unwrap();
		let _client_stream = connector.connect(server_name, tcp).await.unwrap();

		let (_, info) = server.await.unwrap();
		assert_eq!(info.sni.as_deref(), Some("localhost"));
		assert!(info.tls_version.is_some());
	}

	#[tokio::test]
	async fn sni_selects_correct_cert() {
		let (cert_pem_a, key_pem_a) = generate_self_signed(vec!["alpha.example.com".to_owned()]);
		let (cert_pem_b, key_pem_b) = generate_self_signed(vec!["beta.example.com".to_owned()]);

		let loaded_a = parse_pem(&cert_pem_a, &key_pem_a).unwrap();
		let loaded_b = parse_pem(&cert_pem_b, &key_pem_b).unwrap();

		// Keep expected cert for comparison after handshake
		let expected_cert = loaded_a.certs()[0].clone();

		let mut store = CertStore::new();
		store.insert("alpha.example.com", loaded_a);
		store.insert("beta.example.com", loaded_b);

		let server_config = build_server_config(Arc::new(store), &[]).unwrap();

		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let sc = server_config.clone();
		let server = tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			accept_tls(stream, &sc, Duration::from_secs(5)).await.unwrap()
		});

		let client_config = build_test_client_config(&[]);
		let connector = TlsConnector::from(client_config);
		let tcp = TcpStream::connect(addr).await.unwrap();
		let server_name = ServerName::try_from("alpha.example.com").unwrap();
		let client_stream = connector.connect(server_name, tcp).await.unwrap();

		let (_, client_conn) = client_stream.get_ref();
		let peer_certs = client_conn.peer_certificates().unwrap();
		assert_eq!(peer_certs[0], expected_cert);

		let (_, info) = server.await.unwrap();
		assert_eq!(info.sni.as_deref(), Some("alpha.example.com"));
	}

	#[tokio::test]
	async fn handshake_timeout() {
		let (cert_pem, key_pem) = generate_self_signed(vec!["localhost".to_owned()]);
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();

		let mut store = CertStore::new();
		store.insert("default", loaded);

		let server_config = build_server_config(Arc::new(store), &[]).unwrap();

		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		// Connect TCP but never send TLS ClientHello
		let _client = TcpStream::connect(addr).await.unwrap();

		let (stream, _) = listener.accept().await.unwrap();
		let result = accept_tls(stream, &server_config, Duration::from_millis(100)).await;

		assert!(matches!(result, Err(TlsAcceptError::HandshakeTimeout { .. })));
	}

	#[tokio::test]
	async fn alpn_negotiation() {
		let (cert_pem, key_pem) = generate_self_signed(vec!["localhost".to_owned()]);
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();

		let mut store = CertStore::new();
		store.insert("default", loaded);

		let server_config =
			build_server_config(Arc::new(store), &["h2".to_owned(), "http/1.1".to_owned()]).unwrap();

		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let sc = server_config.clone();
		let server = tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			accept_tls(stream, &sc, Duration::from_secs(5)).await.unwrap()
		});

		let client_config = build_test_client_config(&["h2"]);
		let connector = TlsConnector::from(client_config);
		let tcp = TcpStream::connect(addr).await.unwrap();
		let server_name = ServerName::try_from("localhost").unwrap();
		let _client_stream = connector.connect(server_name, tcp).await.unwrap();

		let (_, info) = server.await.unwrap();
		assert_eq!(info.alpn.as_deref(), Some("h2"));
	}

	#[test]
	fn build_server_config_empty_store() {
		let store = Arc::new(CertStore::new());
		let result = build_server_config(store, &[]);
		assert!(matches!(result, Err(TlsAcceptError::NoCertificateConfigured)));
	}

	#[tokio::test]
	async fn non_tls_data_handshake_fails() {
		use tokio::io::AsyncWriteExt;

		let (cert_pem, key_pem) = generate_self_signed(vec!["localhost".to_owned()]);
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();

		let mut store = CertStore::new();
		store.insert("default", loaded);

		let server_config = build_server_config(Arc::new(store), &[]).unwrap();

		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let client_handle = tokio::spawn(async move {
			let mut stream = TcpStream::connect(addr).await.unwrap();
			// Send HTTP bytes instead of TLS ClientHello
			stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
			// Keep connection open briefly so server can attempt handshake
			tokio::time::sleep(Duration::from_millis(200)).await;
		});

		let (stream, _) = listener.accept().await.unwrap();
		let result = accept_tls(stream, &server_config, Duration::from_secs(5)).await;
		assert!(matches!(result, Err(TlsAcceptError::HandshakeFailed(_))));

		let _ = client_handle.await;
	}

	#[tokio::test]
	async fn sni_fallback_to_default_cert() {
		let (cert_pem, key_pem) = generate_self_signed(vec!["localhost".to_owned()]);
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();

		let mut store = CertStore::new();
		store.insert("default", loaded);

		let server_config = build_server_config(Arc::new(store), &[]).unwrap();

		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let sc = server_config.clone();
		let server = tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			accept_tls(stream, &sc, Duration::from_secs(5)).await.unwrap()
		});

		let client_config = build_test_client_config(&[]);
		let connector = TlsConnector::from(client_config);
		let tcp = TcpStream::connect(addr).await.unwrap();
		// Connect with SNI that doesn't match any cert name — should fall back to "default"
		let server_name = ServerName::try_from("unknown.example.com").unwrap();
		let _client_stream = connector.connect(server_name, tcp).await.unwrap();

		let (_, info) = server.await.unwrap();
		assert_eq!(info.sni.as_deref(), Some("unknown.example.com"));
		assert!(info.tls_version.is_some());
	}
}
