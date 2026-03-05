/* src/transport/src/l4p/quic/muxer.rs */

use super::session::{self, SessionAction};
use super::virtual_socket::{VirtualPacket, VirtualUdpSocket};
use fancy_log::{LogLevel, log};
use quinn::{ConnectionId, ConnectionIdGenerator};
use rand::Rng;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_rustls::rustls;
use vane_app::l7::http::h3;
use vane_primitives::certs;
use vane_primitives::common::sys::lifecycle::{Error, Result};

/// Global QUIC Muxer Manager
pub struct QuicMuxer {
	// Bounded sender for backpressure
	tx: mpsc::Sender<VirtualPacket>,
	// Track last activity for GC
	last_active: Mutex<Instant>,
}

// Use Arc (Strong Reference) to persist Muxer state across packets
static MUXER_REGISTRY: std::sync::OnceLock<Mutex<HashMap<u16, Arc<QuicMuxer>>>> =
	std::sync::OnceLock::new();

/// Custom CID Generator to ensure L4 Compatibility.
#[derive(Debug)]
struct VaneCidGenerator {
	port: u16,
}

impl ConnectionIdGenerator for VaneCidGenerator {
	fn generate_cid(&mut self) -> ConnectionId {
		let mut bytes = [0u8; 8];
		rand::rng().fill(&mut bytes);
		let cid = ConnectionId::new(&bytes);

		session::register_session(
			bytes.to_vec(),
			SessionAction::Terminate { muxer_port: self.port, last_seen: Instant::now(), _guard: None },
		);

		cid
	}

	fn cid_len(&self) -> usize {
		8
	}

	fn cid_lifetime(&self) -> Option<Duration> {
		None
	}
}

impl QuicMuxer {
	/// Get or create a muxer for given port
	pub fn get_or_create(port: u16, cert_sni: &str, physical_socket: Arc<UdpSocket>) -> Arc<Self> {
		let registry = MUXER_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
		let mut map = registry.lock().unwrap();

		if let Some(muxer) = map.get(&port) {
			// Update activity timestamp
			if let Ok(mut t) = muxer.last_active.lock() {
				*t = Instant::now();
			}
			return muxer.clone();
		}

		// Garbage Collection: Remove old muxers (> 5 min idle)
		// This prevents memory leaks in long-running processes
		let now = Instant::now();
		map.retain(|_, muxer| {
			if let Ok(t) = muxer.last_active.lock() {
				now.duration_since(*t).as_secs() < 300
			} else {
				true
			}
		});

		// Create new Muxer
		let muxer = Arc::new(Self::new(port, cert_sni, physical_socket));
		map.insert(port, muxer.clone());

		muxer
	}

	fn new(port: u16, cert_sni: &str, physical_socket: Arc<UdpSocket>) -> Self {
		log(LogLevel::Info, &format!("➜ Initializing QUIC Muxer (Virtual Socket) for port {port}"));

		let channel_cap = envflag::get::<usize>("QUIC_VIRTUAL_CHANNEL_CAPACITY", 1024);

		let (tx, rx) = mpsc::channel::<VirtualPacket>(channel_cap);
		let cert_id = cert_sni.to_owned();

		tokio::spawn(async move {
			let mut endpoint_config = quinn::EndpointConfig::default();
			endpoint_config.cid_generator(move || Box::new(VaneCidGenerator { port }));

			let server_config = match Self::build_server_config(&cert_id) {
				Ok(c) => c,
				Err(e) => {
					log(LogLevel::Error, &format!("✗ Failed to build QUIC config: {e}"));
					return;
				}
			};

			let local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
			let virtual_socket = Arc::new(VirtualUdpSocket::new(rx, physical_socket, local_addr));

			let endpoint = match quinn::Endpoint::new_with_abstract_socket(
				endpoint_config,
				Some(server_config),
				virtual_socket,
				Arc::new(quinn::TokioRuntime),
			) {
				Ok(e) => e,
				Err(e) => {
					log(LogLevel::Error, &format!("✗ Failed to create QUIC endpoint: {e}"));
					return;
				}
			};

			log(LogLevel::Info, &format!("✓ QUIC Endpoint initialized (port {port})"));

			while let Some(incoming) = endpoint.accept().await {
				tokio::spawn(async move {
					match incoming.await {
						Ok(conn) => {
							if let Err(e) = h3::handle_connection(conn).await {
								log(LogLevel::Error, &format!("✗ H3 Engine Error: {e:#}"));
							}
						}
						Err(e) => log(LogLevel::Warn, &format!("⚠ QUIC Handshake Error: {e}")),
					}
				});
			}
		});

		Self { tx, last_active: Mutex::new(Instant::now()) }
	}

	fn build_server_config(cert_id: &str) -> Result<quinn::ServerConfig> {
		let cert = certs::arcswap::get_certificate(cert_id)
			.ok_or_else(|| Error::Configuration("Certificate not found".to_owned()))?;

		let mut crypto = rustls::ServerConfig::builder()
			.with_no_client_auth()
			.with_single_cert(cert.certs.clone(), cert.key_clone()?)
			.map_err(|e| Error::Tls(e.to_string()))?;

		crypto.alpn_protocols = vec![b"h3".to_vec()];

		let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
			quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
				.map_err(|e| Error::Tls(e.to_string()))?,
		));

		let mut transport = quinn::TransportConfig::default();
		transport.max_idle_timeout(
			std::time::Duration::from_secs(30).try_into().ok().map(Some).unwrap_or(None),
		);
		transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
		server_config.transport_config(Arc::new(transport));

		Ok(server_config)
	}

	pub fn feed_packet(
		&self,
		data: bytes::Bytes,
		src_addr: SocketAddr,
		dst_addr: SocketAddr,
	) -> Result<()> {
		let packet = VirtualPacket { data, src_addr, dst_addr };

		// Drop packet if channel is full
		match self.tx.try_send(packet) {
			Ok(_) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
			Err(mpsc::error::TrySendError::Closed(_)) => {
				Err(vane_primitives::common::sys::lifecycle::Error::System(
					"QUIC Muxer channel closed".to_owned(),
				))
			}
		}
	}
}
