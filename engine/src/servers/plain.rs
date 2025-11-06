/* engine/src/servers/plain.rs */

use crate::proxy::domain;
use fancy_log::{LogLevel, log};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

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

	loop {
		match listener.accept().await {
			Ok((socket, peer_addr)) => {
				tokio::spawn(async move {
					handle_connection(socket, peer_addr).await;
				});
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

/// Handles an individual incoming TCP connection.
async fn handle_connection(mut socket: TcpStream, peer_addr: SocketAddr) {
	log(
		LogLevel::Debug,
		&format!("Accepted new plain HTTP connection from: {}", peer_addr),
	);

	let mut buffer = [0; 8192];

	let bytes_read = match socket.read(&mut buffer).await {
		Ok(0) => {
			log(LogLevel::Debug, "Client disconnected before sending data.");
			return;
		}
		Ok(n) => n,
		Err(e) => {
			log(
				LogLevel::Warn,
				&format!("Failed to read from socket: {}", e),
			);
			return;
		}
	};

	let mut headers = [httparse::EMPTY_HEADER; 64];
	let mut req = httparse::Request::new(&mut headers);

	match req.parse(&buffer[..bytes_read]) {
		Ok(httparse::Status::Complete(_)) => {
			let version = if req.version == Some(0) {
				"HTTP/1.0"
			} else {
				"HTTP/1.1"
			};

			let host_opt = req
				.headers
				.iter()
				.find(|h| h.name.eq_ignore_ascii_case("Host"))
				.and_then(|h| std::str::from_utf8(h.value).ok())
				.map(|s| s.split(':').next().unwrap_or(s));

			if let Some(host) = host_opt {
				let domains = domain::get_domain_list();
				let matched_domain = match_domain(host, &domains);
				log(
					LogLevel::Debug,
					&format!(
						"{} request for host '{}' matched to domain '{}'",
						version, host, matched_domain
					),
				);
			} else {
				log(
					LogLevel::Warn,
					&format!("{} request received with no Host header.", version),
				);

				log(LogLevel::Debug, "Hard RST has been triggered.");

				// --- FIX: Set SO_LINGER to 0 to force a hard reset (RST) on close. ---
				if let Err(e) = socket.set_linger(Some(Duration::from_secs(0))) {
					log(
						LogLevel::Warn,
						&format!("Failed to set linger option: {}", e),
					);
				}

				// Now, when the function returns, the socket drop will be abrupt.
				return;
			}

			// TODO: Hand off the connection to the full request/response handler.
		}
		_ => {
			log(
				LogLevel::Warn,
				"Received a malformed or incomplete HTTP request.",
			);
		}
	}
}

/// Matches a host against the configured domain list with specific precedence.
fn match_domain<'a>(host: &str, domains: &'a [String]) -> &'a str {
	// Exact match.
	if let Some(domain) = domains.iter().find(|d| d.as_str() == host) {
		return domain;
	}

	// Wildcard match.
	if let Some((_, suffix)) = host.split_once('.') {
		let wildcard_domain = format!("*.{}", suffix);
		if let Some(domain) = domains.iter().find(|d| d.as_str() == wildcard_domain) {
			return domain;
		}
	}

	// Fallback.
	"fallback"
}
