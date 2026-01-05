/* src/plugins/l7/upstream/quic_pool.rs */

use super::tls_verifier::NoVerifier;
use crate::common::config::env_loader;
use crate::common::sys::lifecycle::{Error, Result};
use crate::layers::l4::resolver;
use fancy_log::{LogLevel, log};
use h3::client::SendRequest;
use h3_quinn::{
	OpenStreams,
	quinn::{ClientConfig, Endpoint, TransportConfig},
};
use once_cell::sync::Lazy;
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::{OnceCell, RwLock};

pub type QuicSender = SendRequest<OpenStreams, bytes::Bytes>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey(String, u16, bool);

static GLOBAL_ENDPOINT: OnceCell<Endpoint> = OnceCell::const_new();

async fn get_global_endpoint() -> Result<&'static Endpoint> {
	GLOBAL_ENDPOINT
		.get_or_try_init(|| async {
			let crypto = rustls::ClientConfig::builder()
				.with_root_certificates(rustls::RootCertStore::empty())
				.with_no_client_auth();

			let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
				.map_err(|e| Error::System(format!("QUIC Crypto Config Error: {}", e)))?;
			let mut client_config = ClientConfig::new(Arc::new(quic_config));

			// Get idle timeout from env, default 90s
			let idle_timeout_s = env_loader::get_env("UPSTREAM_POOL_IDLE_TIMEOUT", "90".to_string())
				.parse::<u64>()
				.unwrap_or(90);

			let mut transport = TransportConfig::default();
			transport.max_idle_timeout(Some(
				Duration::from_secs(idle_timeout_s)
					.try_into()
					.map_err(|e| Error::System(format!("Invalid idle timeout duration: {}", e)))?,
			));
			transport.keep_alive_interval(Some(Duration::from_secs(10)));
			client_config.transport_config(Arc::new(transport));

			let addr = "0.0.0.0:0"
				.parse()
				.map_err(|e| Error::System(format!("Failed to parse bind address for QUIC: {}", e)))?;

			let mut endpoint = Endpoint::client(addr)
				.map_err(|e| Error::System(format!("Failed to bind QUIC Endpoint: {}", e)))?;
			endpoint.set_default_client_config(client_config);

			log(
				LogLevel::Debug,
				&format!(
					"➜ QUIC Global Endpoint Initialized (0.0.0.0:0) | IdleTimeout: {}s",
					idle_timeout_s
				),
			);
			Ok(endpoint)
		})
		.await
}

static CONNECTION_POOL: Lazy<RwLock<HashMap<PoolKey, QuicSender>>> =
	Lazy::new(|| RwLock::new(HashMap::new()));

pub async fn get_or_create_connection(
	host: &str,
	port: u16,
	skip_verify: bool,
) -> Result<QuicSender> {
	let key = PoolKey(host.to_string(), port, skip_verify);

	{
		let pool = CONNECTION_POOL.read().await;
		if let Some(sender) = pool.get(&key) {
			return Ok(sender.clone());
		}
	}

	let mut pool = CONNECTION_POOL.write().await;
	if let Some(sender) = pool.get(&key) {
		return Ok(sender.clone());
	}

	log(
		LogLevel::Debug,
		&format!(
			"➜ FetchUpstream H3 Establishing new QUIC connection to {}:{}",
			host, port
		),
	);
	let sender = connect_internal(host, port, skip_verify).await?;
	pool.insert(key, sender.clone());

	Ok(sender)
}

async fn connect_internal(host: &str, port: u16, skip_verify: bool) -> Result<QuicSender> {
	let ips = resolver::resolve_domain_to_ips(host).await;
	let ip = ips
		.first()
		.ok_or_else(|| Error::System(format!("DNS lookup returned no IPs for host: {}", host)))?;

	let addr = SocketAddr::new(*ip, port);

	let crypto = build_rustls_config(skip_verify)?;
	let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
		.map_err(|e| Error::System(format!("TLS Config Error: {}", e)))?;

	let client_config = ClientConfig::new(Arc::new(quic_crypto));

	let endpoint = get_global_endpoint().await?;

	let connection = endpoint
		.connect_with(client_config, addr, host)
		.map_err(|e| Error::System(format!("QUIC Connect Failed: {}", e)))?
		.await
		.map_err(|e| Error::System(format!("QUIC Handshake Failed: {}", e)))?;

	let quinn_conn = h3_quinn::Connection::new(connection);
	let (mut driver, send_request) = h3::client::new(quinn_conn)
		.await
		.map_err(|e| Error::System(format!("H3 Handshake Failed: {}", e)))?;

	let key_clone = PoolKey(host.to_string(), port, skip_verify);

	// Monitor connection lifecycle
	tokio::spawn(async move {
		// Fix: wait_idle() returns Error directly, not Result
		let e = driver.wait_idle().await;
		log(
			LogLevel::Warn,
			&format!(
				"⚠ QUIC Connection lost for {}:{}: {}",
				key_clone.0, key_clone.1, e
			),
		);
		let mut pool = CONNECTION_POOL.write().await;
		pool.remove(&key_clone);
	});

	Ok(send_request)
}

fn build_rustls_config(skip_verify: bool) -> Result<rustls::ClientConfig> {
	let mut config = if skip_verify {
		let mut c = rustls::ClientConfig::builder()
			.with_root_certificates(rustls::RootCertStore::empty())
			.with_no_client_auth();
		c.dangerous().set_certificate_verifier(Arc::new(NoVerifier));
		c
	} else {
		let mut roots = rustls::RootCertStore::empty();
		let result = rustls_native_certs::load_native_certs();

		if !result.errors.is_empty() {
			log(
				LogLevel::Warn,
				&format!(
					"⚠ Encountered {} errors loading system certs.",
					result.errors.len()
				),
			);
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
