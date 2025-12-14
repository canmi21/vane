/* src/modules/stack/protocol/carrier/tls.rs */

use super::{context, flow};
use crate::common::getenv;
use crate::modules::{
	kv::KvStore,
	plugins::{model::ConnectionObject, protocol::tls::clienthello},
	stack::protocol::carrier::model::RESOLVER_REGISTRY,
};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use tokio::net::TcpStream;

/// Entry point for TLS L4+ flows.
pub async fn run(stream: TcpStream, kv: &mut KvStore, parent_path: String) -> Result<()> {
	log(LogLevel::Debug, "➜ Entering TLS L4+ Resolver...");

	let buffer_size_str = getenv::get_env("TLS_CLIENTHELLO_BUFFER_SIZE", "4096".to_string());
	let buffer_size = buffer_size_str.parse::<usize>().unwrap_or(4096);

	let mut buf = vec![0u8; buffer_size];

	match stream.peek(&mut buf).await {
		Ok(n) => {
			log(
				LogLevel::Debug,
				&format!("⚙ Socket peek returned {} bytes.", n),
			);
			if n > 0 {
				let payload = &buf[..n];
				let clienthello_hex = hex::encode(payload);
				kv.insert("tls.clienthello".to_string(), clienthello_hex);

				match clienthello::parse_client_hello(payload) {
					Ok(data) => {
						context::inject_tls_data(kv, data);
					}
					Err(e) => {
						// Use {:#} to show full error chain if parsing fails
						log(
							LogLevel::Warn,
							&format!("⚠ Failed to parse ClientHello (len={}): {:#}", n, e),
						);
					}
				}
			} else {
				log(
					LogLevel::Debug,
					"⚙ Socket peek returned 0 bytes (Empty/Closed).",
				);
			}
		}
		Err(e) => {
			log(LogLevel::Warn, &format!("✗ Failed to peek socket: {}", e));
		}
	}

	let conn = ConnectionObject::Stream(Box::new(stream));
	context::inject_common(kv, "tls");

	let registry = RESOLVER_REGISTRY.load();
	let config = registry
		.get("tls")
		.ok_or_else(|| anyhow!("No resolver config found for 'tls'"))?;

	if let Err(e) = flow::execute(&config.connection, kv, conn, parent_path).await {
		// FIXED: Use {:#} to print the full error chain (Context + Root Cause)
		log(
			LogLevel::Error,
			&format!("✗ TLS Flow execution failed: {:#}", e),
		);
		return Err(e);
	}

	Ok(())
}
