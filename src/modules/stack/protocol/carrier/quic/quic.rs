/* src/modules/stack/protocol/carrier/quic/quic.rs */

use super::muxer::QuicMuxer;
use super::session::{self, PendingState, SessionAction};
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
use std::collections::BTreeMap;
use std::time::Instant;

pub async fn run(conn: ConnectionObject, kv: &mut KvStore, parent_path: String) -> Result<()> {
	// Extract UDP socket info
	let (socket_arc, client_addr, dst_addr, datagram) = match &conn {
		ConnectionObject::Udp {
			socket,
			client_addr,
			datagram,
		} => {
			let dst_addr = socket.local_addr()?;
			(socket.clone(), *client_addr, dst_addr, datagram.clone())
		}
		_ => return Err(anyhow!("QUIC carrier requires UDP connection object")),
	};

	context::inject_common(kv, "quic");

	// Initial Lightweight Parse to get DCID and Crypto Frames
	let limit_str = getenv::get_env("QUIC_LONG_HEADER_BUFFER_SIZE", "4096".to_string());
	let max_len = limit_str.parse::<usize>().unwrap_or(4096);
	let parse_len = std::cmp::min(datagram.len(), max_len);

	let parsed_packet = match parser::parse_initial_packet(&datagram[..parse_len]) {
		Ok(p) => p,
		Err(_) => {
			// If parsing fails (Short Header/Handshake), check IP:PORT sticky map.
			// Change here, Pls use the upstream socket from sticky map to avoid EINVAL/NAT breakage
			if let Some((target, upstream_socket)) = session::get_sticky(&client_addr) {
				// Blind forward to sticky target using correct source port
				log(
					LogLevel::Debug,
					&format!("➜ Sticky Forward: {} -> {}", client_addr, target),
				);
				let _ = upstream_socket.send_to(&datagram, target).await;
			}
			return Ok(());
		}
	};

	let dcid_bytes = hex::decode(&parsed_packet.dcid).unwrap_or_default();
	if dcid_bytes.is_empty() {
		return Ok(());
	}

	// Buffer Management & Stream Reassembly
	let mut sni_found = parsed_packet.sni_hint.clone();
	let mut should_proceed = false;

	let max_pending_packets = getenv::get_env("QUIC_MAX_PENDING_PACKETS", "5".to_string())
		.parse::<usize>()
		.unwrap_or(5);

	// Critical: Lock the pending map to update state
	{
		let mut entry = session::PENDING_INITIALS
			.entry(dcid_bytes.clone())
			.or_insert(PendingState {
				crypto_stream: BTreeMap::new(),
				queued_packets: Vec::new(),
				last_seen: Instant::now(),
				processing: false,
			});

		// 1. If already being processed by another task, just buffer and return
		if entry.processing {
			entry
				.queued_packets
				.push((datagram.clone(), client_addr, dst_addr));
			return Ok(());
		}

		entry
			.queued_packets
			.push((datagram.clone(), client_addr, dst_addr));
		entry.last_seen = Instant::now();

		for (offset, data) in parsed_packet.crypto_frames {
			entry.crypto_stream.insert(offset, data);
		}

		// 2. Attempt SNI reassembly if not yet found
		if sni_found.is_none() && !entry.crypto_stream.is_empty() {
			let mut full_stream = Vec::new();
			let mut expected_offset = 0;

			for (offset, data) in &entry.crypto_stream {
				if *offset == expected_offset {
					full_stream.extend_from_slice(data);
					expected_offset += data.len();
				}
			}

			if let Ok(reassembled_sni) = parser::parse_tls_client_hello_sni(&full_stream) {
				log(
					LogLevel::Debug,
					&format!(
						"✓ Reassembled SNI from {} fragments: {}",
						entry.crypto_stream.len(),
						reassembled_sni
					),
				);
				sni_found = Some(reassembled_sni);
			}
		}

		// 3. Decide whether to proceed to flow or keep buffering
		if let Some(sni) = &sni_found {
			// SNI is ready! Mark as processing to prevent other fragments from entering flow
			entry.processing = true;
			should_proceed = true;
			// We need a clone here because we will use it outside the lock
			sni_found = Some(sni.clone());
		} else {
			// Still waiting for SNI. Check for buffer overflow or timeout.
			if entry.queued_packets.len() >= max_pending_packets {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ QUIC buffer limit reached ({} pkts) for DCID {} without SNI. Dropping.",
						max_pending_packets, parsed_packet.dcid
					),
				);
				drop(entry); // Release reference before removal
				session::PENDING_INITIALS.remove(&dcid_bytes);
			} else {
				log(
					LogLevel::Debug,
					&format!(
						"⚙ Buffered QUIC packet (pkts={}). Waiting for SNI...",
						entry.queued_packets.len()
					),
				);
			}
		}
	}

	// 4. Return if we don't have enough data yet or if another task took over
	if !should_proceed {
		return Ok(());
	}

	let sni = sni_found
		.ok_or_else(|| anyhow!("QUIC logic error: should_proceed is true but SNI is missing"))?;

	context::inject_quic_data(
		kv,
		parser::QuicInitialData {
			sni_hint: Some(sni.clone()),
			dcid: parsed_packet.dcid.clone(),
			scid: parsed_packet.scid.clone(),
			version: parsed_packet.version.clone(),
			token: parsed_packet.token.clone(),
			crypto_frames: vec![], // Not needed for context
		},
	);

	let registry = RESOLVER_REGISTRY.load();
	let config = registry
		.get("quic")
		.ok_or_else(|| anyhow!("No resolver config found for 'quic'"))?;

	let execution_result = flow::execute(&config.connection, kv, conn, parent_path).await;

	// Apply Decision & Flush Buffer
	match execution_result {
		Ok(TerminatorResult::Finished) => {
			if let Some((_, _state)) = session::PENDING_INITIALS.remove(&dcid_bytes) {
				log(
					LogLevel::Debug,
					"⚙ Forwarding flow finished. (Pending queue flushed/dropped)",
				);
			}
			Ok(())
		}
		Ok(TerminatorResult::Upgrade { protocol, .. }) => {
			if protocol == "httpx" {
				let cert_sni = kv
					.get("tls.termination.cert_sni")
					.map(|s| s.as_str())
					.unwrap_or("default");

				let local_port = socket_arc.local_addr()?.port();
				let muxer = QuicMuxer::get_or_create(local_port, cert_sni, socket_arc.clone());

				log(
					LogLevel::Debug,
					&format!(
						"⚙ Registering QUIC Session (Terminator) for DCID len={}",
						dcid_bytes.len()
					),
				);
				session::register_session(
					dcid_bytes.clone(),
					SessionAction::Terminate {
						muxer_port: local_port,
						last_seen: Instant::now(),
					},
				);

				if let Some((_, state)) = session::PENDING_INITIALS.remove(&dcid_bytes) {
					log(
						LogLevel::Debug,
						&format!(
							"➜ Flushing {} buffered packets to H3 Muxer",
							state.queued_packets.len()
						),
					);
					for (data, c_addr, d_addr) in state.queued_packets {
						muxer.feed_packet(data, c_addr, d_addr)?;
					}
				} else {
					muxer.feed_packet(datagram, client_addr, dst_addr)?;
				}

				Ok(())
			} else {
				Err(anyhow!("Unsupported QUIC upgrade target: {}", protocol))
			}
		}
		Err(e) => Err(e),
	}
}
