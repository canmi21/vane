/* src/modules/stack/protocol/carrier/tls.rs */

use super::{context, flow};
use crate::common::getenv;
use crate::modules::{
	kv::KvStore, plugins::model::ConnectionObject, stack::protocol::carrier::model::RESOLVER_REGISTRY,
};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use tokio::net::TcpStream;

/// Entry point for TLS L4+ flows.
/// Captures ClientHello, injects context, and executes the configured flow.
pub async fn run(stream: TcpStream, kv: &mut KvStore, parent_path: String) -> Result<()> {
	log(LogLevel::Debug, "➜ Entering TLS L4+ Resolver...");

	// 1. Capture ClientHello (Peek)
	// Use environment variable for buffer size, default to 4096 bytes.
	let buffer_size_str = getenv::get_env("TLS_CLIENTHELLO_BUFFER_SIZE", "4096".to_string());
	let buffer_size = buffer_size_str.parse::<usize>().unwrap_or(4096);

	let mut buf = vec![0u8; buffer_size];

	match stream.peek(&mut buf).await {
		Ok(n) if n > 0 => {
			let clienthello_hex = hex::encode(&buf[..n]);
			// Inject raw ClientHello into KV for plugins (SNI/ALPN) to use
			kv.insert("tls.clienthello".to_string(), clienthello_hex);
		}
		Ok(_) => {
			log(
				LogLevel::Debug,
				"⚙ TLS connection closed or empty during peek.",
			);
		}
		Err(e) => {
			log(
				LogLevel::Warn,
				&format!("✗ Failed to peek TLS ClientHello: {}", e),
			);
		}
	}

	// 2. Wrap Connection
	let conn = ConnectionObject::Stream(Box::new(stream));

	// 3. Inject Common Context
	context::inject_common(kv, "tls");

	// 4. Load Config
	let registry = RESOLVER_REGISTRY.load();
	let config = registry
		.get("tls")
		.ok_or_else(|| anyhow!("No resolver config found for 'tls'"))?;

	// 5. Execute Flow
	if let Err(e) = flow::execute(&config.connection, kv, conn, parent_path).await {
		log(
			LogLevel::Error,
			&format!("✗ TLS Flow execution failed: {}", e),
		);
		return Err(e);
	}

	Ok(())
}
