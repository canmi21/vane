/* src/modules/stack/transport/context.rs */

use crate::common::getenv;
use crate::modules::kv::KvStore;
use tokio::net::TcpStream;

/// Peeks at the initial bytes of a TCP stream and populates the KvStore with context data.
///
/// It reads `TCP_DETECT_LIMIT` to determine how many bytes to peek.
/// Upon success, it writes:
/// - `req.peek_buffer_hex`: The hex-encoded initial payload.
/// - `conn.proto`: The underlying transport protocol ("tcp").
///
/// Returns the number of bytes successfully peeked.
pub async fn populate_tcp_context(
	socket: &mut TcpStream,
	kv: &mut KvStore,
) -> std::io::Result<usize> {
	// 1. Determine peek limit from environment
	let limit_str = getenv::get_env("TCP_DETECT_LIMIT", "64".to_string());
	let limit = limit_str.parse::<usize>().unwrap_or(64);
	const MAX_DETECT_LIMIT: usize = 8192;
	let final_limit = limit.min(MAX_DETECT_LIMIT);
	let mut buf = vec![0u8; final_limit];

	// 2. Peek data
	let n = socket.peek(&mut buf).await?;

	// 3. Populate KvStore if data exists
	if n > 0 {
		let payload_hex = hex::encode(&buf[..n]);
		kv.insert("req.peek_buffer_hex".to_string(), payload_hex);
		kv.insert("conn.proto".to_string(), "tcp".to_string());
	}

	Ok(n)
}
