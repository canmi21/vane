/* src/modules/plugins/upstream/pool.rs */

use crate::common::getenv;
use crate::common::requirements::Error;
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use once_cell::sync::Lazy;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{
	ClientConfig, DigitallySignedStruct,
	pki_types::{CertificateDer, ServerName, UnixTime},
};
use std::sync::Arc;
use std::time::Duration;

// --- Custom Verifier for Skip SSL ---

#[derive(Debug)]
struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
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

	fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
		vec![
			rustls::SignatureScheme::RSA_PKCS1_SHA1,
			rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
			rustls::SignatureScheme::RSA_PSS_SHA256,
			rustls::SignatureScheme::ED25519,
		]
	}
}

// --- Client Types ---

pub type HttpClient = Client<HttpsConnector<HttpConnector>, BoxBody<Bytes, Error>>;

// --- Global Pools ---

pub static GLOBAL_SECURE_CLIENT: Lazy<HttpClient> = Lazy::new(|| build_client(false));
pub static GLOBAL_INSECURE_CLIENT: Lazy<HttpClient> = Lazy::new(|| build_client(true));

fn build_client(skip_verify: bool) -> HttpClient {
	let idle_timeout_s = getenv::get_env("UPSTREAM_POOL_IDLE_TIMEOUT", "90".to_string())
		.parse::<u64>()
		.unwrap_or(90);

	let max_idle = getenv::get_env("UPSTREAM_POOL_MAX_IDLE", "32".to_string())
		.parse::<usize>()
		.unwrap_or(32);

	let keepalive_s = getenv::get_env("UPSTREAM_KEEPALIVE_INTERVAL", "30".to_string())
		.parse::<u64>()
		.unwrap_or(30);

	// With 'http2' feature enabled in Cargo.toml, enable_all_versions() is available.
	// This allows ALPN negotiation to choose between H1 and H2 automatically.

	let https_connector = if skip_verify {
		let mut config = ClientConfig::builder()
			.with_root_certificates(rustls::RootCertStore::empty())
			.with_no_client_auth();

		config
			.dangerous()
			.set_certificate_verifier(Arc::new(NoVerifier));

		hyper_rustls::HttpsConnectorBuilder::new()
			.with_tls_config(config)
			.https_or_http()
			.enable_all_versions()
			.build()
	} else {
		hyper_rustls::HttpsConnectorBuilder::new()
			.with_native_roots()
			.expect("Failed to load native roots")
			.https_or_http()
			.enable_all_versions()
			.build()
	};

	Client::builder(TokioExecutor::new())
		.pool_idle_timeout(Duration::from_secs(idle_timeout_s))
		.pool_max_idle_per_host(max_idle)
		.http2_keep_alive_interval(Some(Duration::from_secs(keepalive_s)))
		.build(https_connector)
}
