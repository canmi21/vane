/* src/layers/l4p/tls.rs */

use super::{context, flow};
use crate::common::config::env_loader;
use crate::common::sys::lifecycle::{Error, Result};
use crate::engine::interfaces::{ConnectionObject, TerminatorResult};
use crate::plugins::protocol::tls::clienthello;
use crate::plugins::protocol::upgrader::decryptor;
use crate::resources::kv::KvStore;
use anyhow::anyhow;
use fancy_log::{LogLevel, log};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Entry point for TLS L4+ flows.
/// Handles ClientHello parsing, L4+ routing, and L7 Handover.
pub async fn run(stream: TcpStream, kv: &mut KvStore, parent_path: String) -> Result<()> {
	log(LogLevel::Debug, "➜ Entering TLS L4+ Resolver...");

	let buffer_size_str = env_loader::get_env("TLS_CLIENTHELLO_BUFFER_SIZE", "4096".to_owned());
	let buffer_size = buffer_size_str.parse::<usize>().unwrap_or(4096);

	let peek_timeout_ms = env_loader::get_env("TLS_HANDSHAKE_PEEK_TIMEOUT_MS", "500".to_owned())
		.parse::<u64>()
		.unwrap_or(500);

	let allow_parse_failure =
		env_loader::get_env("TLS_ALLOW_PARSE_FAILURE", "false".to_owned()).to_lowercase() == "true";

	let mut buf = vec![0u8; buffer_size];
	let mut parse_success = false;
	let mut error_code = None;
	let mut initial_payloads = ahash::AHashMap::new();

	// 1. Smart Peek Loop (Handles fragmentation)
	let peek_result = timeout(Duration::from_millis(peek_timeout_ms), async {
		loop {
			match stream.peek(&mut buf).await {
				Ok(n) if n >= 5 => {
					// Check if it's a Handshake (0x16)
					if buf[0] != 0x16 {
						return Err("not_tls");
					}
					// Calculate expected length from TLS header (bytes 3-4)
					let record_len = ((buf[3] as usize) << 8) | (buf[4] as usize);
					let total_expected = 5 + record_len;

					if n >= total_expected {
						// Full record available
						return Ok(n);
					}

					if total_expected > buffer_size {
						return Err("buffer_too_small");
					}

					// Wait a bit for more data
					tokio::time::sleep(Duration::from_millis(10)).await;
				}
				Ok(0) => return Err("closed"),
				Ok(_) => {
					// Less than 5 bytes, wait
					tokio::time::sleep(Duration::from_millis(10)).await;
				}
				Err(_) => return Err("io_error"),
			}
		}
	})
	.await;

	match peek_result {
		Ok(Ok(n)) => {
			log(
				LogLevel::Debug,
				&format!("⚙ Socket peek returned full record ({n} bytes)."),
			);
			let payload = &buf[..n];

			// LAZY: Store raw bytes instead of eager hex encode
			initial_payloads.insert(
				"tls.clienthello".to_owned(),
				bytes::Bytes::copy_from_slice(payload),
			);

			match clienthello::parse_client_hello(payload) {
				Ok(data) => {
					context::inject_tls_data(kv, data);
					parse_success = true;
				}
				Err(e) => {
					log(
						LogLevel::Warn,
						&format!("⚠ Failed to parse ClientHello: {e:#}"),
					);
					error_code = Some("malformed");
				}
			}
		}
		Ok(Err(code)) => {
			log(LogLevel::Warn, &format!("⚠ TLS Peek failed: {code}"));
			error_code = Some(code);
		}
		Err(_) => {
			log(
				LogLevel::Warn,
				"⚠ TLS Peek timed out waiting for handshake.",
			);
			error_code = Some("timeout");
		}
	}

	if !parse_success {
		if let Some(err) = error_code {
			kv.insert("tls.error".to_owned(), err.to_owned());
		}

		if allow_parse_failure {
			kv.insert("tls.sni".to_owned(), "unknown".to_owned());
			log(
				LogLevel::Warn,
				"⚠ TLS inspection failed, continuing with 'unknown' context (TLS_ALLOW_PARSE_FAILURE=true)",
			);
		} else {
			log(
				LogLevel::Error,
				&format!(
					"✗ TLS inspection failed ({}). Dropping connection (Fail-Closed).",
					error_code.unwrap_or("unknown")
				),
			);
			return Err(Error::System(format!(
				"TLS inspection failed: {}.",
				error_code.unwrap_or("unknown")
			)));
		}
	}

	// 2. Prepare Connection & Context
	let conn = ConnectionObject::Stream(Box::new(stream));
	context::inject_common(kv, "tls");

	let config_manager = crate::config::get();
	let config = config_manager
		.resolvers
		.get("tls")
		.ok_or_else(|| anyhow!("No resolver config found for 'tls'"))?;

	// 3. Execute L4+ Flow
	// We MUST capture the result to handle Upgrades (Handover).
	let result = flow::execute(&config.connection, kv, conn, parent_path, initial_payloads)
		.await
		.map_err(|e| {
			log(
				LogLevel::Error,
				&format!("✗ TLS Flow execution failed: {e:#}"),
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
			if protocol.as_str() == "httpx" {
				log(
					LogLevel::Info,
					&format!("➜ Handing over to Decryptor for L7 protocol: {protocol}"),
				);
				decryptor::terminate_and_handover(conn, kv, protocol)
					.await
					.map_err(|e| Error::System(format!("TLS Termination Error: {e:#}")))
			} else {
				log(
					LogLevel::Error,
					&format!("✗ Unsupported L4+ Upgrade Target: {protocol}"),
				);
				Err(Error::Configuration(format!(
					"Unknown/Unsupported protocol upgrade: {protocol}"
				)))
			}
		}
	}
}
