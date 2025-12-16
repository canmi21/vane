/* src/modules/stack/protocol/carrier/quic/muxer.rs */

use crate::common::requirements::{Error, Result};
use crate::modules::{certs, stack::protocol::application::h3};
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_rustls::rustls;

/// Global QUIC State Manager
pub struct QuicMuxer {
	tx: mpsc::Sender<QuicPacket>,
}

struct QuicPacket {
	data: Vec<u8>,
	#[allow(dead_code)]
	client_addr: SocketAddr,
}

static MUXER_REGISTRY: std::sync::OnceLock<Mutex<HashMap<u16, Arc<QuicMuxer>>>> =
	std::sync::OnceLock::new();

impl QuicMuxer {
	pub fn get_or_create(port: u16, cert_sni: &str) -> Arc<Self> {
		let registry = MUXER_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
		let mut map = registry.lock().unwrap();

		if let Some(muxer) = map.get(&port) {
			return muxer.clone();
		}

		let muxer = Arc::new(Self::new(port, cert_sni));
		map.insert(port, muxer.clone());
		muxer
	}

	fn new(port: u16, cert_sni: &str) -> Self {
		log(
			LogLevel::Info,
			&format!(
				"➜ Initializing QUIC Muxer (Virtual Endpoint) for port {}",
				port
			),
		);

		let (tx, mut rx) = mpsc::channel::<QuicPacket>(1024);
		let cert_id = cert_sni.to_string();

		tokio::spawn(async move {
			let server_config = match Self::build_server_config(&cert_id) {
				Ok(c) => c,
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Failed to build QUIC config: {}", e),
					);
					return;
				}
			};

			let endpoint = match quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()) {
				Ok(e) => e,
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Failed to bind internal QUIC endpoint: {}", e),
					);
					return;
				}
			};

			let local_addr = endpoint.local_addr().unwrap();
			log(
				LogLevel::Debug,
				&format!("⚙ Internal QUIC Endpoint listening on {}", local_addr),
			);

			let feeder_socket = match UdpSocket::bind("127.0.0.1:0").await {
				Ok(s) => s,
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Failed to bind feeder socket: {}", e),
					);
					return;
				}
			};

			let feeder = Arc::new(feeder_socket);
			let feeder_clone = feeder.clone();

			tokio::spawn(async move {
				while let Some(packet) = rx.recv().await {
					if let Err(e) = feeder_clone.send_to(&packet.data, local_addr).await {
						log(
							LogLevel::Warn,
							&format!("⚠ Failed to feed packet to QUIC endpoint: {}", e),
						);
					}
				}
			});

			while let Some(incoming) = endpoint.accept().await {
				log(LogLevel::Debug, "➜ New QUIC Connection Incoming...");
				tokio::spawn(async move {
					match incoming.await {
						Ok(conn) => {
							log(LogLevel::Debug, "✓ QUIC Handshake Complete. Starting H3...");
							if let Err(e) = h3::handle_connection(conn).await {
								log(LogLevel::Error, &format!("✗ H3 Engine Error: {:#}", e));
							}
						}
						Err(e) => {
							log(LogLevel::Warn, &format!("✗ QUIC Handshake Error: {}", e));
						}
					}
				});
			}
		});

		Self { tx }
	}

	fn build_server_config(cert_id: &str) -> Result<quinn::ServerConfig> {
		let cert = certs::arcswap::get_certificate(cert_id)
			.ok_or_else(|| Error::Configuration(format!("Certificate '{}' not found", cert_id)))?;

		// Build rustls ServerConfig with explicit crypto provider
		let crypto = rustls::ServerConfig::builder_with_provider(Arc::new(
			rustls::crypto::ring::default_provider(),
		))
		.with_safe_default_protocol_versions()
		.map_err(|e| Error::Tls(format!("Failed to set protocol versions: {}", e)))?
		.with_no_client_auth()
		.with_single_cert(cert.certs.clone(), cert.key_clone())
		.map_err(|e: rustls::Error| Error::Tls(e.to_string()))?;

		// Set ALPN for H3
		let mut crypto_with_alpn = crypto;
		crypto_with_alpn.alpn_protocols = vec![b"h3".to_vec()];

		// Convert to Quinn ServerConfig
		let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
			quinn::crypto::rustls::QuicServerConfig::try_from(crypto_with_alpn)
				.map_err(|e| Error::Tls(format!("Failed to create QuicServerConfig: {}", e)))?,
		));

		// Configure transport parameters
		let mut transport_config = quinn::TransportConfig::default();
		transport_config.max_concurrent_bidi_streams(100u32.into());
		transport_config.max_concurrent_uni_streams(100u32.into());
		server_config.transport_config(Arc::new(transport_config));

		Ok(server_config)
	}

	pub async fn feed_packet(&self, data: Vec<u8>, client_addr: SocketAddr) {
		if self
			.tx
			.send(QuicPacket { data, client_addr })
			.await
			.is_err()
		{
			log(
				LogLevel::Debug,
				"⚙ QUIC Muxer channel closed, dropping packet.",
			);
		}
	}
}
