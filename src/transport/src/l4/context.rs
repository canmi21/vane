/* src/transport/src/l4/context.rs */

use tokio::net::TcpStream;
use vane_primitives::kv::KvStore;

/// Peeks at the initial bytes of a TCP stream and populates the KvStore with context data.
pub async fn populate_tcp_context(
	socket: &mut TcpStream,
	kv: &mut KvStore,
) -> std::io::Result<usize> {
	let limit = envflag::get::<usize>("TCP_DETECT_LIMIT", 64);
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
	let limit = envflag::get::<usize>("UDP_DETECT_LIMIT", 64);
	let len = datagram.len().min(limit);

	if len > 0 {
		let payload_hex = hex::encode(&datagram[..len]);
		kv.insert("req.peek_buffer_hex".to_owned(), payload_hex);
	}
	kv.insert("conn.proto".to_owned(), "udp".to_owned());
}
