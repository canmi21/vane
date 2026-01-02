/* src/modules/stack/carrier/tls.rs */

use super::{context, flow};
use crate::common::getenv;
use crate::common::requirements::{Error, Result};
use crate::modules::{
	kv::KvStore,
	plugins::{
		core::model::{ConnectionObject, TerminatorResult},
		protocol::tls::clienthello,
		terminators::upgrader::decryptor,
	},
	stack::carrier::model::RESOLVER_REGISTRY,
};
use anyhow::anyhow;
use fancy_log::{LogLevel, log};
use tokio::net::TcpStream;

/// Entry point for TLS L4+ flows.
/// Handles ClientHello parsing, L4+ routing, and L7 Handover.
pub async fn run(stream: TcpStream, kv: &mut KvStore, parent_path: String) -> Result<()> {
	log(LogLevel::Debug, "➜ Entering TLS L4+ Resolver...");

	let buffer_size_str = getenv::get_env("TLS_CLIENTHELLO_BUFFER_SIZE", "4096".to_string());
	let buffer_size = buffer_size_str.parse::<usize>().unwrap_or(4096);

	let mut buf = vec![0u8; buffer_size];

	// 1. Peek ClientHello
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

	// 2. Prepare Connection & Context
	let conn = ConnectionObject::Stream(Box::new(stream));
	context::inject_common(kv, "tls");

	let registry = RESOLVER_REGISTRY.load();
	let config = registry
		.get("tls")
		.ok_or_else(|| anyhow!("No resolver config found for 'tls'"))?;

	// 3. Execute L4+ Flow
	// We MUST capture the result to handle Upgrades (Handover).
	let result = flow::execute(&config.connection, kv, conn, parent_path)
		.await
		.map_err(|e| {
			log(
				LogLevel::Error,
				&format!("✗ TLS Flow execution failed: {:#}", e),
			);
			e
		})?;

	// 4. Handle Flow Result
	match result {
		TerminatorResult::Finished => {
			log(
				LogLevel::Debug,
				"✓ TLS L4+ Flow finished (Connection Closed).",
			);
			Ok(())
		}
		TerminatorResult::Upgrade {
			protocol,
			conn,
			parent_path: _,
		} => {
			// Connects L4+ to L7.
			match protocol.as_str() {
				"httpx" => {
					log(
						LogLevel::Info,
						&format!("➜ Handing over to Decryptor for L7 protocol: {}", protocol),
					);
					decryptor::terminate_and_handover(conn, kv, protocol)
						.await
						.map_err(|e| Error::System(format!("TLS Termination Error: {:#}", e)))
				}
				_ => {
					log(
						LogLevel::Error,
						&format!("✗ Unsupported L4+ Upgrade Target: {}", protocol),
					);
					Err(Error::Configuration(format!(
						"Unknown/Unsupported protocol upgrade: {}",
						protocol
					)))
				}
			}
		}
	}
}
