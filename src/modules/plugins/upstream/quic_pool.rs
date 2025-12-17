/* src/modules/plugins/upstream/quic_pool.rs */

use super::tls_verifier::NoVerifier;
use crate::common::requirements::{Error, Result};
use crate::modules::stack::transport::resolver;
use fancy_log::{LogLevel, log};
use h3::client::SendRequest;
use h3_quinn::{
	OpenStreams,
	quinn::{ClientConfig, Endpoint, TransportConfig},
};
use once_cell::sync::Lazy;
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;

// Type definition for the H3 Sender
pub type QuicSender = SendRequest<OpenStreams, bytes::Bytes>;

// Cache Key: (Host, Port, SkipVerify)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey(String, u16, bool);

// --- Global Pools ---

// The Global Endpoint (Physical Socket)
static GLOBAL_ENDPOINT: Lazy<Endpoint> = Lazy::new(|| {
	// Default client config (will be overridden per connection, but Endpoint needs a default)
	let crypto = rustls::ClientConfig::builder()
		.with_root_certificates(rustls::RootCertStore::empty())
		.with_no_client_auth();

	let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap();
	let mut client_config = ClientConfig::new(Arc::new(quic_config));

	let mut transport = TransportConfig::default();
	transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
	transport.keep_alive_interval(Some(Duration::from_secs(10)));
	client_config.transport_config(Arc::new(transport));

	let mut endpoint =
		Endpoint::client("0.0.0.0:0".parse().unwrap()).expect("Failed to bind QUIC Endpoint");
	endpoint.set_default_client_config(client_config);

	log(
		LogLevel::Debug,
		"➜ QUIC Global Endpoint Initialized (0.0.0.0:0)",
	);
	endpoint
});

// The Connection Pool
// We use a RwLock for thread safety.
static CONNECTION_POOL: Lazy<RwLock<HashMap<PoolKey, QuicSender>>> =
	Lazy::new(|| RwLock::new(HashMap::new()));

/// Public API: Get a valid H3 Sender (Multiplexed Stream Creator)
/// This function handles connection establishment, pooling, and verification configuration.
pub async fn get_or_create_connection(
	host: &str,
	port: u16,
	skip_verify: bool,
) -> Result<QuicSender> {
	let key = PoolKey(host.to_string(), port, skip_verify);

	// 1. Try Read Lock (Fast Path)
	{
		let pool = CONNECTION_POOL.read().await;
		if let Some(sender) = pool.get(&key) {
			return Ok(sender.clone());
		}
	} // Drop Read Lock

	// 2. Write Lock (Slow Path - Connect)
	// We need to re-check presence inside write lock to avoid race conditions
	let mut pool = CONNECTION_POOL.write().await;
	if let Some(sender) = pool.get(&key) {
		return Ok(sender.clone());
	}

	// 3. Establish New Connection
	log(
		LogLevel::Debug,
		&format!(
			"➜ FetchUpstream H3 Establishing new QUIC connection to {}:{}",
			host, port
		),
	);
	let sender = connect_internal(host, port, skip_verify).await?;

	// 4. Store in Pool
	pool.insert(key, sender.clone());

	Ok(sender)
}

/// Internal helper to perform the actual QUIC Handshake + H3 Setup
async fn connect_internal(host: &str, port: u16, skip_verify: bool) -> Result<QuicSender> {
	// A. DNS Lookup (Using Vane's Custom Resolver)
	// We use the existing logic in transport/resolver.rs which respects NAMESERVER env vars.
	let ips = resolver::resolve_domain_to_ips(host).await;

	let ip = ips
		.first()
		.ok_or_else(|| Error::System(format!("DNS lookup returned no IPs for host: {}", host)))?;

	let addr = SocketAddr::new(*ip, port);

	// B. TLS Configuration
	let crypto = build_rustls_config(skip_verify)?;
	let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
		.map_err(|e| Error::System(format!("TLS Config Error: {}", e)))?;

	let client_config = ClientConfig::new(Arc::new(quic_crypto));

	// C. QUIC Connect
	let connection = GLOBAL_ENDPOINT
		.connect_with(client_config, addr, host)
		.map_err(|e| Error::System(format!("QUIC Connect Failed: {}", e)))?
		.await
		.map_err(|e| Error::System(format!("QUIC Handshake Failed: {}", e)))?;

	// D. H3 Handshake
	let quinn_conn = h3_quinn::Connection::new(connection);
	let (mut driver, send_request) = h3::client::new(quinn_conn)
		.await
		.map_err(|e| Error::System(format!("H3 Handshake Failed: {}", e)))?;

	// E. Spawn Driver (The Background Actor)
	// If this driver dies, the connection is dead.
	let key_clone = PoolKey(host.to_string(), port, skip_verify);
	tokio::spawn(async move {
		// FIXED: driver.wait_idle() returns Error directly, not Result<T, E>
		let e = driver.wait_idle().await;
		log(
			LogLevel::Warn,
			&format!(
				"⚠ QUIC Connection lost for {}:{}: {}",
				key_clone.0, key_clone.1, e
			),
		);
		// Cleanup from pool when connection dies
		let mut pool = CONNECTION_POOL.write().await;
		pool.remove(&key_clone);
	});

	Ok(send_request)
}

// --- TLS Config Logic (System Certs included) ---
fn build_rustls_config(skip_verify: bool) -> Result<rustls::ClientConfig> {
	let mut config = if skip_verify {
		let mut c = rustls::ClientConfig::builder()
			.with_root_certificates(rustls::RootCertStore::empty())
			.with_no_client_auth();
		c.dangerous().set_certificate_verifier(Arc::new(NoVerifier));
		c
	} else {
		// Load Native Certs
		let mut roots = rustls::RootCertStore::empty();

		// FIXED: load_native_certs returns CertificateResult { certs, errors, .. }
		let result = rustls_native_certs::load_native_certs();

		if !result.errors.is_empty() {
			log(
				LogLevel::Warn,
				&format!(
					"⚠ Encountered {} errors loading system certs.",
					result.errors.len()
				),
			);
			// Debug log individual errors if needed
			for err in result.errors {
				log(LogLevel::Debug, &format!("  Cert Load Error: {}", err));
			}
		}

		if result.certs.is_empty() {
			log(
				LogLevel::Warn,
				"⚠ No system certificates found. HTTPS might fail.",
			);
		}

		for cert in result.certs {
			if let Err(e) = roots.add(cert) {
				log(
					LogLevel::Warn,
					&format!("⚠ Failed to add a system root cert: {}", e),
				);
			}
		}

		rustls::ClientConfig::builder()
			.with_root_certificates(roots)
			.with_no_client_auth()
	};

	config.alpn_protocols = vec![b"h3".to_vec()];
	Ok(config)
}
