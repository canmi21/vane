/* engine/src/servers/mod.rs */

pub mod plain;
pub mod quic;
pub mod tls;

use fancy_log::{LogLevel, log};
use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

const DEFAULT_HTTP_PORT: u16 = 80;
const DEFAULT_HTTPS_PORT: u16 = 443;
const BIND_ADDR: IpAddr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));

/// Reads environment variables and starts all proxy servers concurrently.
pub async fn start_proxy_servers() {
	// Get HTTP port from .env or use default.
	let http_port = env::var("BIND_HTTP_PORT")
		.ok()
		.and_then(|s| s.parse::<u16>().ok())
		.unwrap_or(DEFAULT_HTTP_PORT);

	// Get HTTPS/QUIC port from .env or use default.
	let https_port = env::var("BIND_HTTPS_PORT")
		.ok()
		.and_then(|s| s.parse::<u16>().ok())
		.unwrap_or(DEFAULT_HTTPS_PORT);

	log(
		LogLevel::Info,
		&format!(
			"Proxy listeners configured for HTTP: {}, HTTPS/TLS: {}, QUIC/H3: {}",
			http_port, https_port, https_port
		),
	);

	let http_addr = SocketAddr::new(BIND_ADDR, http_port);
	let https_addr = SocketAddr::new(BIND_ADDR, https_port);

	// Spawn each server on its own async task.
	tokio::spawn(plain::start(http_addr));
	tokio::spawn(tls::start(https_addr));
	tokio::spawn(quic::start(https_addr));
}
