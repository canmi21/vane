/* src/layers/l4/context.rs */

use crate::common::config::env_loader;
use crate::resources::kv::KvStore;
use tokio::net::TcpStream;

/// Peeks at the initial bytes of a TCP stream and populates the KvStore with context data.
pub async fn populate_tcp_context(
	socket: &mut TcpStream,
	kv: &mut KvStore,
) -> std::io::Result<usize> {
	let limit_str = env_loader::get_env("TCP_DETECT_LIMIT", "64".to_owned());
	let limit = limit_str.parse::<usize>().unwrap_or(64);
	const MAX_DETECT_LIMIT: usize = 8192;
	let final_limit = limit.min(MAX_DETECT_LIMIT);
	let mut buf = vec![0u8; final_limit];

	let n = socket.peek(&mut buf).await?;

	if n > 0 {
		let payload_hex = hex::encode(&buf[..n]);
		kv.insert("req.peek_buffer_hex".to_owned(), payload_hex);
		kv.insert("conn.proto".to_owned(), "tcp".to_owned());
	}

	Ok(n)
}

/// Populates the KvStore with context data from a UDP datagram.
/// Unlike TCP, UDP data is already read, so we just encode the prefix.
pub fn populate_udp_context(datagram: &[u8], kv: &mut KvStore) {
	let limit_str = env_loader::get_env("UDP_DETECT_LIMIT", "64".to_owned());
	let limit = limit_str.parse::<usize>().unwrap_or(64);
	let len = datagram.len().min(limit);

	if len > 0 {
		let payload_hex = hex::encode(&datagram[..len]);
		kv.insert("req.peek_buffer_hex".to_owned(), payload_hex);
	}
	kv.insert("conn.proto".to_owned(), "udp".to_owned());
}
