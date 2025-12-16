/* src/modules/stack/protocol/carrier/quic/muxer.rs */

use super::virtual_socket::{VirtualPacket, VirtualUdpSocket};
use crate::common::getenv;
use crate::common::requirements::{Error, Result};
use crate::modules::{certs, stack::protocol::application::http::h3};
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, Weak};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_rustls::rustls;

/// Global QUIC Muxer Manager
pub struct QuicMuxer {
	// Bounded sender for backpressure
	tx: mpsc::Sender<VirtualPacket>,
}

// Registry stores Weak references to allow auto-cleanup when Muxer is dropped
static MUXER_REGISTRY: std::sync::OnceLock<Mutex<HashMap<u16, Weak<QuicMuxer>>>> =
	std::sync::OnceLock::new();

impl QuicMuxer {
	/// Get or create a muxer for given port
	pub fn get_or_create(port: u16, cert_sni: &str, physical_socket: Arc<UdpSocket>) -> Arc<Self> {
		let registry = MUXER_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
		let mut map = registry.lock().unwrap();

		// Optimization: Lazy Garbage Collection
		// Only iterate the map if it grows beyond a threshold configured via env
		let gc_threshold_str = getenv::get_env("QUIC_MUXER_GC_THRESHOLD", "64".to_string());
		let gc_threshold = gc_threshold_str.parse::<usize>().unwrap_or(64);

		if map.len() > gc_threshold {
			map.retain(|_, weak| weak.strong_count() > 0);
		}

		if let Some(weak) = map.get(&port) {
			if let Some(muxer) = weak.upgrade() {
				return muxer;
			}
		}

		// Create new Muxer
		let muxer = Arc::new(Self::new(port, cert_sni, physical_socket));
		map.insert(port, Arc::downgrade(&muxer));

		muxer
	}

	fn new(port: u16, cert_sni: &str, physical_socket: Arc<UdpSocket>) -> Self {
		log(
			LogLevel::Info,
			&format!(
				"➜ Initializing QUIC Muxer (Virtual Socket) for port {}",
				port
			),
		);

		// Use BOUNDED channel for backpressure.
		// Capacity configured via env (Default: 1024 packets).
		let channel_cap_str = getenv::get_env("QUIC_VIRTUAL_CHANNEL_CAPACITY", "1024".to_string());
		let channel_cap = channel_cap_str.parse::<usize>().unwrap_or(1024);

		let (tx, rx) = mpsc::channel::<VirtualPacket>(channel_cap);
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

			let local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
			let virtual_socket = Arc::new(VirtualUdpSocket::new(rx, physical_socket, local_addr));
			let endpoint_config = quinn::EndpointConfig::default();

			// Note: Ensure `quinn` feature `runtime-tokio` is enabled
			let endpoint = match quinn::Endpoint::new_with_abstract_socket(
				endpoint_config,
				Some(server_config),
				virtual_socket,
				Arc::new(quinn::TokioRuntime),
			) {
				Ok(e) => e,
				Err(e) => {
					log(
						LogLevel::Error,
						&format!("✗ Failed to create QUIC endpoint: {}", e),
					);
					return;
				}
			};

			log(
				LogLevel::Info,
				&format!("✓ QUIC Endpoint initialized (port {})", port),
			);

			while let Some(incoming) = endpoint.accept().await {
				tokio::spawn(async move {
					match incoming.await {
						Ok(conn) => {
							if let Err(e) = h3::handle_connection(conn).await {
								log(LogLevel::Error, &format!("✗ H3 Engine Error: {:#}", e));
							}
						}
						Err(e) => log(LogLevel::Warn, &format!("✗ QUIC Handshake Error: {}", e)),
					}
				});
			}
		});

		Self { tx }
	}

	fn build_server_config(cert_id: &str) -> Result<quinn::ServerConfig> {
		let cert = certs::arcswap::get_certificate(cert_id)
			.ok_or_else(|| Error::Configuration(format!("Certificate not found")))?;

		let mut crypto = rustls::ServerConfig::builder()
			.with_no_client_auth()
			.with_single_cert(cert.certs.clone(), cert.key_clone())
			.map_err(|e| Error::Tls(e.to_string()))?;

		crypto.alpn_protocols = vec![b"h3".to_vec()];

		let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
			quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
				.map_err(|e| Error::Tls(e.to_string()))?,
		));

		// Important: Keep alive settings match Virtual Socket constraints
		let mut transport = quinn::TransportConfig::default();
		transport.max_idle_timeout(Some(std::time::Duration::from_secs(30).try_into().unwrap()));
		transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
		server_config.transport_config(Arc::new(transport));

		Ok(server_config)
	}

	/// Feed a packet. Now async-aware or lossy.
	/// Since we are in the hot path of the Dispatcher, we should try_send.
	/// If full, we DROP the packet (UDP behavior).
	pub fn feed_packet(
		&self,
		data: Vec<u8>,
		src_addr: SocketAddr,
		dst_addr: SocketAddr,
	) -> Result<()> {
		let packet = VirtualPacket {
			data: bytes::Bytes::from(data), // Convert to Bytes
			src_addr,
			dst_addr,
		};

		// try_send returns error if full.
		// We ignore Full error (packet drop) but log/return System error for closed channel.
		match self.tx.try_send(packet) {
			Ok(_) => Ok(()),
			Err(mpsc::error::TrySendError::Full(_)) => {
				// Backpressure active: Queue full, dropping packet.
				// Optionally log metric here (don't log to file per packet!)
				Ok(())
			}
			Err(mpsc::error::TrySendError::Closed(_)) => {
				Err(Error::System("QUIC Muxer channel closed".to_string()))
			}
		}
	}
}
