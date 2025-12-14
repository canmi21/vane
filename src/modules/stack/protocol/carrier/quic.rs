/* src/modules/stack/protocol/carrier/quic.rs */

use super::{context, flow};
use crate::common::getenv;
use crate::modules::{
	kv::KvStore,
	plugins::{model::ConnectionObject, protocol::quic::parser},
	stack::protocol::carrier::model::RESOLVER_REGISTRY,
};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};

/// Entry point for QUIC L4+ flows.
///
/// Handles the "upgraded" UDP flow. Since UDP is connectionless, "conn" here
/// represents the session context. We rely on the `req.peek_buffer_hex` injected
/// by the L4 listener/dispatcher to parse the Initial packet.
pub async fn run(conn: ConnectionObject, kv: &mut KvStore, parent_path: String) -> Result<()> {
	log(LogLevel::Debug, "➜ Entering QUIC L4+ Resolver...");

	context::inject_common(kv, "quic");

	// 1. Retrieve the Initial Packet Data from KV (injected by L4/UDP context populator)
	if let Some(hex_data) = kv.get("req.peek_buffer_hex") {
		// Respect the QUIC specific buffer size limit for safety
		let limit_str = getenv::get_env("QUIC_LONG_HEADER_BUFFER_SIZE", "4096".to_string());
		let max_len = limit_str.parse::<usize>().unwrap_or(4096);

		match hex::decode(hex_data) {
			Ok(data) => {
				let parse_len = std::cmp::min(data.len(), max_len);
				let payload = &data[..parse_len];

				// 2. Parse QUIC Headers
				match parser::parse_initial_packet(payload) {
					Ok(parsed_data) => {
						context::inject_quic_data(kv, parsed_data);
					}
					Err(e) => {
						// Use {:#} to show full error chain
						log(
							LogLevel::Warn,
							&format!("⚠ Failed to parse QUIC Initial packet: {:#}", e),
						);
					}
				}
			}
			Err(e) => {
				log(
					LogLevel::Error,
					&format!("✗ Failed to decode peek buffer from hex: {}", e),
				);
			}
		}
	} else {
		log(
			LogLevel::Warn,
			"⚠ No 'req.peek_buffer_hex' found in Context. Cannot parse QUIC headers.",
		);
	}

	// 3. Load Resolver Config
	let registry = RESOLVER_REGISTRY.load();
	let config = registry
		.get("quic")
		.ok_or_else(|| anyhow!("No resolver config found for 'quic'"))?;

	// 4. Execute Flow
	if let Err(e) = flow::execute(&config.connection, kv, conn, parent_path).await {
		log(
			LogLevel::Error,
			&format!("✗ QUIC Flow execution failed: {:#}", e),
		);
		return Err(e);
	}

	Ok(())
}
