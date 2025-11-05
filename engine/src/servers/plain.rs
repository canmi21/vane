/* engine/src/servers/plain.rs */

use fancy_log::{LogLevel, log};
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// Starts a TCP listener for plain HTTP traffic on the given address.
pub async fn start(addr: SocketAddr) {
	log(
		LogLevel::Info,
		&format!("Starting plain HTTP server on {}...", addr),
	);

	let listener = match TcpListener::bind(addr).await {
		Ok(l) => l,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to bind plain HTTP listener on {}: {}", addr, e),
			);
			return;
		}
	};

	// Loop to accept incoming connections.
	// The actual request handling logic will be added here later.
	loop {
		match listener.accept().await {
			Ok((_socket, peer_addr)) => {
				log(
					LogLevel::Debug,
					&format!("Accepted new plain HTTP connection from: {}", peer_addr),
				);
				// TODO: Hand off the connection to the router/handler.
			}
			Err(e) => {
				log(
					LogLevel::Warn,
					&format!("Error accepting plain HTTP connection: {}", e),
				);
			}
		}
	}
}
