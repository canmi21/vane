/* src/transport/src/l4p/quic/protocol.rs */

use super::muxer::QuicMuxer;
use super::session::{self, PendingState, SessionAction};
use crate::l4p::{context, flow};
use crate::protocol::quic::parser;
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use std::collections::BTreeMap;
use std::time::Instant;
use vane_engine::engine::interfaces::{ConnectionObject, TerminatorResult};
use vane_primitives::kv::KvStore;
use vane_primitives::tasks::GLOBAL_TRACKER;

pub async fn run(conn: ConnectionObject, kv: &mut KvStore, parent_path: String) -> Result<()> {
	// Extract UDP socket info
	let (socket_arc, client_addr, dst_addr, datagram) = match &conn {
		ConnectionObject::Udp { socket, client_addr, datagram } => {
			let dst_addr = socket.local_addr()?;
			(socket.clone(), *client_addr, dst_addr, datagram.clone())
		}
		_ => return Err(anyhow!("QUIC carrier requires UDP connection object")),
	};

	context::inject_common(kv, "quic");

	// Initial Lightweight Parse to get DCID and Crypto Frames
	let max_len = envflag::get::<usize>("QUIC_LONG_HEADER_BUFFER_SIZE", 4096);
	let parse_len = std::cmp::min(datagram.len(), max_len);

	let Ok(parsed_packet) = parser::parse_initial_packet(&datagram[..parse_len]) else {
		// If parsing fails (Short Header/Handshake), check IP:PORT sticky map.
		if let Some((target, upstream_socket)) = session::get_sticky(&client_addr) {
			// Blind forward to sticky target using correct source port
			log(LogLevel::Debug, &format!("➜ Sticky Forward: {client_addr} -> {target}"));
			let _ = upstream_socket.send_to(&datagram, target).await;
		}
		return Ok(());
	};

	let dcid_bytes = hex::decode(&parsed_packet.dcid).unwrap_or_default();
	if dcid_bytes.is_empty() {
		return Ok(());
	}

	// Buffer Management & Stream Reassembly
	let mut sni_found = parsed_packet.sni_hint.clone();
	let mut should_proceed = false;

	let max_pending_packets = envflag::get::<usize>("QUIC_MAX_PENDING_PACKETS", 5);

	// Critical: Lock the pending map to update state
	// Scope the entry to ensure the shard lock is released before any .await
	{
		// 0. Pre-check global limits before even locking (optimistic)
		if !session::try_reserve_global_bytes(datagram.len()) {
			return Ok(());
		}

		let mut entry = if let Some(e) = session::PENDING_INITIALS.get_mut(&dcid_bytes) {
			e
		} else {
			// Apply Connection Rate Limits
			let Some(guard) = GLOBAL_TRACKER.acquire(client_addr.ip()) else {
				log(
					LogLevel::Debug,
					&format!(
						"⚙ Rate limited QUIC session from {} (DCID {})",
						client_addr, parsed_packet.dcid
					),
				);
				// Release bytes since we aren't storing
				session::release_global_bytes(datagram.len());
				return Ok(());
			};

			session::PENDING_INITIALS.entry(dcid_bytes.clone()).or_insert(PendingState {
				crypto_stream: BTreeMap::new(),
				queued_packets: Vec::new(),
				last_seen: Instant::now(),
				processing: false,
				_guard: guard,
				total_bytes: 0,
			})
		};

		// 1. Check Session Limits
		if !session::check_session_limit(entry.total_bytes, datagram.len()) {
			// Release the reserved bytes since we reject this packet
			session::release_global_bytes(datagram.len());
			return Ok(());
		}

		// 2. If already being processed by another task, buffer and return
		if entry.processing {
			entry.total_bytes += datagram.len();
			entry.queued_packets.push((datagram.clone(), client_addr, dst_addr));
			return Ok(());
		}

		// Update stats and queue
		entry.total_bytes += datagram.len();
		entry.queued_packets.push((datagram.clone(), client_addr, dst_addr));
		entry.last_seen = Instant::now();

		entry.crypto_stream.extend(parsed_packet.crypto_frames);

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

	let mut initial_payloads = ahash::AHashMap::new();
	// LAZY: Store raw datagram for {{quic.initial}} hijacking
	initial_payloads.insert("quic.initial".to_owned(), bytes::Bytes::copy_from_slice(&datagram));

	context::inject_quic_data(
		kv,
		parser::QuicInitialData {
			sni_hint: Some(sni.clone()),
			dcid: parsed_packet.dcid.clone(),
			scid: parsed_packet.scid.clone(),
			version: parsed_packet.version.clone(),
			token: parsed_packet.token.clone(),
			crypto_frames: BTreeMap::new(), // Not needed for context
		},
	);

	let config_manager = vane_engine::config::get();
	let config = config_manager
		.resolvers
		.get("quic")
		.ok_or_else(|| anyhow!("No resolver config found for 'quic'"))?;

	let execution_result =
		flow::execute(&config.connection, kv, conn, parent_path, initial_payloads).await;

	// Apply Decision & Flush Buffer
	match execution_result {
		Ok(TerminatorResult::Finished) => {
			if let Some((_, _state)) = session::PENDING_INITIALS.remove(&dcid_bytes) {
				log(LogLevel::Debug, "⚙ Forwarding flow finished. (Pending queue flushed/dropped)");
			}
			Ok(())
		}
		Ok(TerminatorResult::Upgrade { protocol, .. }) => {
			if protocol == "httpx" {
				let cert_sni = kv.get("tls.termination.cert_sni").map(|s| s.as_str()).unwrap_or("default");

				let local_port = socket_arc.local_addr()?.port();
				let muxer = QuicMuxer::get_or_create(local_port, cert_sni, socket_arc.clone());

				log(
					LogLevel::Debug,
					&format!("⚙ Registering QUIC Session (Terminator) for DCID len={}", dcid_bytes.len()),
				);

				// 1. Retrieve guard from Pending state (Clone it to keep pending entry valid for now)
				let guard = if let Some(entry) = session::PENDING_INITIALS.get(&dcid_bytes) {
					entry._guard.clone()
				} else {
					// Fallback: Acquire new guard if state is missing (rare race)
					match GLOBAL_TRACKER.acquire(client_addr.ip()) {
						Some(g) => g,
						None => return Ok(()),
					}
				};

				// 2. Register Session (Action becomes active immediately)
				session::register_session(
					dcid_bytes.clone(),
					SessionAction::Terminate {
						muxer_port: local_port,
						last_seen: Instant::now(),
						_guard: Some(guard),
					},
				);

				// 3. Remove and Flush Pending State
				if let Some((_, mut state)) = session::PENDING_INITIALS.remove(&dcid_bytes) {
					let packets = state.drain_queue();
					log(
						LogLevel::Debug,
						&format!("➜ Flushing {} buffered packets to H3 Muxer", packets.len()),
					);
					for (data, c_addr, d_addr) in packets {
						muxer.feed_packet(data, c_addr, d_addr)?;
					}
				} else {
					// If no pending state (e.g. this was the very first packet and processed immediately)
					muxer.feed_packet(datagram.clone(), client_addr, dst_addr)?;
				}

				Ok(())
			} else {
				Err(anyhow!("Unsupported QUIC upgrade target: {protocol}"))
			}
		}
		Err(e) => Err(e),
	}
}
