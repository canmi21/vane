/* src/modules/stack/protocol/carrier/quic/quic.rs */

// FIXED: Import paths from carrier parent module
use super::muxer::QuicMuxer;
use crate::common::getenv;
use crate::modules::stack::protocol::carrier::{context, flow};
use crate::modules::{
	kv::KvStore,
	plugins::{
		model::{ConnectionObject, TerminatorResult},
		protocol::quic::parser,
	},
	stack::protocol::carrier::model::RESOLVER_REGISTRY,
};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};

pub async fn run(conn: ConnectionObject, kv: &mut KvStore, parent_path: String) -> Result<()> {
	// 1. Unwrap UDP Socket info
	let (socket_arc, client_addr, datagram) = match &conn {
		ConnectionObject::Udp {
			socket,
			client_addr,
			datagram,
		} => (socket.clone(), *client_addr, datagram.clone()),
		_ => return Err(anyhow!("QUIC carrier requires UDP connection object")),
	};

	context::inject_common(kv, "quic");

	// 2. Parse Initial Packet
	if let Some(hex_data) = kv.get("req.peek_buffer_hex") {
		let limit_str = getenv::get_env("QUIC_LONG_HEADER_BUFFER_SIZE", "4096".to_string());
		let max_len = limit_str.parse::<usize>().unwrap_or(4096);

		if let Ok(data) = hex::decode(hex_data) {
			let parse_len = std::cmp::min(data.len(), max_len);
			match parser::parse_initial_packet(&data[..parse_len]) {
				Ok(parsed_data) => context::inject_quic_data(kv, parsed_data),
				Err(_) => {}
			}
		}
	}

	// 3. Load Resolver Config
	let registry = RESOLVER_REGISTRY.load();
	let config = registry
		.get("quic")
		.ok_or_else(|| anyhow!("No resolver config found for 'quic'"))?;

	// 4. Execute Flow
	let execution_result = flow::execute(&config.connection, kv, conn, parent_path).await;

	match execution_result {
		Ok(TerminatorResult::Finished) => Ok(()),
		Ok(TerminatorResult::Upgrade { protocol, .. }) => {
			if protocol == "h3" {
				// 5. Handover to Muxer
				let cert_sni = kv
					.get("tls.termination.cert_sni")
					.map(|s| s.as_str())
					.unwrap_or("default");
				let local_port = socket_arc.local_addr()?.port();

				let muxer = QuicMuxer::get_or_create(local_port, cert_sni);

				// Feed the initial packet to establish context in the Muxer
				muxer.feed_packet(datagram, client_addr).await;

				log(LogLevel::Debug, "✓ QUIC Initial packet fed to H3 Engine.");
				Ok(())
			} else {
				Err(anyhow!("Unsupported QUIC upgrade target: {}", protocol))
			}
		}
		Err(e) => Err(e),
	}
}
