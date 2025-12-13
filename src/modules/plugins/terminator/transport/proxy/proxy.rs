/* src/modules/plugins/terminator/transport/proxy/proxy.rs */

use crate::modules::stack::transport::model::ResolvedTarget;
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{Duration, timeout};

// Constants
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Proxies a raw TCP stream to a target.
pub async fn proxy_tcp_stream(mut client_stream: TcpStream, target: ResolvedTarget) -> Result<()> {
	// Connect to upstream
	let mut upstream_stream = timeout(
		CONNECT_TIMEOUT,
		TcpStream::connect(format!("{}:{}", target.ip, target.port)),
	)
	.await
	.with_context(|| {
		format!(
			"Timeout connecting to upstream {}:{}",
			target.ip, target.port
		)
	})?
	.with_context(|| {
		format!(
			"Failed to connect to upstream {}:{}",
			target.ip, target.port
		)
	})?;

	// Disable Nagle's algorithm for lower latency
	let _ = client_stream.set_nodelay(true);
	let _ = upstream_stream.set_nodelay(true);

	// Bidirectional copy
	let (mut client_read, mut client_write) = client_stream.split();
	let (mut upstream_read, mut upstream_write) = upstream_stream.split();

	let client_to_server = tokio::io::copy(&mut client_read, &mut upstream_write);
	let server_to_client = tokio::io::copy(&mut upstream_read, &mut client_write);

	// Race the two copy tasks
	tokio::select! {
			res = client_to_server => res.map(|_| ()).map_err(|e| anyhow!("Client->Server copy failed: {}", e)),
			res = server_to_client => res.map(|_| ()).map_err(|e| anyhow!("Server->Client copy failed: {}", e)),
	}
}

/// Proxies a generic ByteStream (e.g., TlsStream) to a target TCP address.
pub async fn proxy_generic_stream(
	client_stream: Box<dyn crate::modules::plugins::model::ByteStream>,
	target: ResolvedTarget,
) -> Result<()> {
	// Connect to upstream (Raw TCP)
	let mut upstream_stream = timeout(
		CONNECT_TIMEOUT,
		TcpStream::connect(format!("{}:{}", target.ip, target.port)),
	)
	.await
	.with_context(|| {
		format!(
			"Timeout connecting to upstream {}:{}",
			target.ip, target.port
		)
	})?
	.with_context(|| {
		format!(
			"Failed to connect to upstream {}:{}",
			target.ip, target.port
		)
	})?;

	let _ = upstream_stream.set_nodelay(true);

	let (mut client_read, mut client_write) = tokio::io::split(client_stream);
	let (mut upstream_read, mut upstream_write) = upstream_stream.split();

	let client_to_server = tokio::io::copy(&mut client_read, &mut upstream_write);
	let server_to_client = tokio::io::copy(&mut upstream_read, &mut client_write);

	tokio::select! {
			res = client_to_server => res.map(|_| ()).map_err(|e| anyhow!("L4+ Client->Server copy failed: {}", e)),
			res = server_to_client => res.map(|_| ()).map_err(|e| anyhow!("L4+ Server->Client copy failed: {}", e)),
	}
}

/// Proxies a single UDP datagram.
pub async fn proxy_udp_direct(
	socket: Arc<UdpSocket>,
	datagram: &[u8],
	_client_addr: std::net::SocketAddr,
	target: ResolvedTarget,
) -> Result<()> {
	// Send data to upstream
	socket
		.send_to(datagram, format!("{}:{}", target.ip, target.port))
		.await
		.with_context(|| format!("Failed to send UDP packet to {}:{}", target.ip, target.port))?;

	Ok(())
}
