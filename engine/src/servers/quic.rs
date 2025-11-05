/* engine/src/servers/quic.rs */

use fancy_log::{LogLevel, log};
use std::net::SocketAddr;
use tokio::net::UdpSocket;

/// Starts a UDP listener for QUIC (HTTP/3) traffic on the given address.
pub async fn start(addr: SocketAddr) {
	log(
		LogLevel::Info,
		&format!("Starting QUIC (H3) server on {}...", addr),
	);

	let socket = match UdpSocket::bind(addr).await {
		Ok(s) => s,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to bind QUIC listener on {}: {}", addr, e),
			);
			return;
		}
	};

	let mut buf = vec![0u8; 65507]; // Max UDP payload size

	// Loop to receive incoming UDP datagrams.
	// The actual QUIC connection management will be added here.
	loop {
		match socket.recv_from(&mut buf).await {
			Ok((len, peer_addr)) => {
				if len > 0 {
					log(
						LogLevel::Debug,
						&format!("Received {} bytes of QUIC data from: {}", len, peer_addr),
					);
					// TODO: Pass the datagram to the QUIC endpoint handler.
				}
			}
			Err(e) => {
				log(
					LogLevel::Warn,
					&format!("Error receiving QUIC datagram: {}", e),
				);
			}
		}
	}
}
