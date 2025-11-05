/* engine/src/servers/tls.rs */

use fancy_log::{LogLevel, log};
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// Starts a TCP listener for TLS (HTTPS) traffic on the given address.
pub async fn start(addr: SocketAddr) {
	log(
		LogLevel::Info,
		&format!("Starting TLS (HTTPS) server on {}...", addr),
	);

	let listener = match TcpListener::bind(addr).await {
		Ok(l) => l,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to bind TLS listener on {}: {}", addr, e),
			);
			return;
		}
	};

	// Loop to accept incoming connections.
	// This will later involve SNI parsing and dynamic certificate handling.
	loop {
		match listener.accept().await {
			Ok((_socket, peer_addr)) => {
				log(
					LogLevel::Debug,
					&format!("Accepted new TLS connection from: {}", peer_addr),
				);
				// TODO: Perform TLS handshake and hand off to the router.
			}
			Err(e) => {
				log(
					LogLevel::Warn,
					&format!("Error accepting TLS connection: {}", e),
				);
			}
		}
	}
}
