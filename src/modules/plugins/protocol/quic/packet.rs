/* src/modules/plugins/protocol/quic/packet.rs */

use anyhow::{Result, anyhow};

use std::collections::BTreeMap;

#[derive(Debug, Default, Clone)]
pub struct QuicInitialData {
	pub version: String,
	pub dcid: String,
	pub scid: String,
	pub token: Option<String>,
	pub sni_hint: Option<String>,
	/// Raw CRYPTO frames extracted from this packet (Offset -> Data)
	pub crypto_frames: BTreeMap<usize, Vec<u8>>,
}

/// Parses a QUIC Long Header Initial Packet.
/// Orchestrates decryption and frame extraction.
pub fn parse_initial_packet(payload: &[u8]) -> Result<QuicInitialData> {
	if payload.len() < 10 {
		return Err(anyhow!("Buffer too short for QUIC Long Header"));
	}

	let first_byte = payload[0];

	// 1. Header Form Check (bit 7 must be 1)
	if (first_byte & 0x80) == 0 {
		return Err(anyhow!("Not a Long Header packet"));
	}

	// 2. Fixed Bit Check (bit 6 must be 1)
	if (first_byte & 0x40) == 0 {
		return Err(anyhow!("Fixed bit not set"));
	}

	// 3. Packet Type Check (Initial is 0x00)
	let packet_type = (first_byte & 0x30) >> 4;
	if packet_type != 0 {
		return Err(anyhow!("Not an Initial Packet (Type: {})", packet_type));
	}

	let mut cursor = 1;

	// 4. Version
	if cursor + 4 > payload.len() {
		return Err(anyhow!("Truncated Version"));
	}
	let version_bytes = &payload[cursor..cursor + 4];
	let version_val = u32::from_be_bytes([
		version_bytes[0],
		version_bytes[1],
		version_bytes[2],
		version_bytes[3],
	]);
	let version = format!("0x{:08x}", version_val);
	cursor += 4;

	// 5. DCID
	if cursor + 1 > payload.len() {
		return Err(anyhow!("Truncated DCID Length"));
	}
	let dcid_len = payload[cursor] as usize;
	cursor += 1;
	if dcid_len > 20 {
		return Err(anyhow!("DCID length {} exceeds 20", dcid_len));
	}
	if cursor + dcid_len > payload.len() {
		return Err(anyhow!("Truncated DCID"));
	}
	let dcid_bytes = &payload[cursor..cursor + dcid_len];
	let dcid = hex::encode(dcid_bytes);
	cursor += dcid_len;

	// 6. SCID
	if cursor + 1 > payload.len() {
		return Err(anyhow!("Truncated SCID Length"));
	}
	let scid_len = payload[cursor] as usize;
	cursor += 1;
	if scid_len > 20 {
		return Err(anyhow!("SCID length {} exceeds 20", scid_len));
	}
	if cursor + scid_len > payload.len() {
		return Err(anyhow!("Truncated SCID"));
	}
	let scid = hex::encode(&payload[cursor..cursor + scid_len]);
	cursor += scid_len;

	// 7. Token
	let (token_len, varint_len) = read_varint(&payload[cursor..])?;
	cursor += varint_len;
	let mut token = None;
	if token_len > 0 {
		if cursor + token_len > payload.len() {
			return Err(anyhow!("Truncated Token"));
		}
		token = Some(hex::encode(&payload[cursor..cursor + token_len]));
		cursor += token_len;
	}

	// 8. Length
	let (remaining_len, varint_len) = read_varint(&payload[cursor..])?;
	cursor += varint_len;
	if cursor + remaining_len > payload.len() {
		return Err(anyhow!("Truncated packet payload"));
	}

	// 9. Crypto & Frame Parsing
	let header_start = 0;
	let protected_payload_start = cursor;

	// Delegate to crypto module
	let (sni_hint, crypto_frames) = super::crypto::extract_decrypted_content(
		payload,
		header_start,
		protected_payload_start,
		remaining_len,
		dcid_bytes,
		version_val,
	)
	.unwrap_or((None, BTreeMap::new()));

	Ok(QuicInitialData {
		version,
		dcid,
		scid,
		token,
		sni_hint,
		crypto_frames,
	})
}

// Helpers for L4 Fast Path
pub fn peek_long_header_dcid(packet: &[u8]) -> Option<Vec<u8>> {
	if packet.len() < 6 {
		return None;
	}
	let dcid_len = packet[5] as usize;
	if dcid_len == 0 || dcid_len > 20 {
		return None;
	}
	if packet.len() < 6 + dcid_len {
		return None;
	}
	Some(packet[6..6 + dcid_len].to_vec())
}

pub fn peek_short_header_dcid(packet: &[u8], len: usize) -> Option<Vec<u8>> {
	if packet.len() < 1 + len {
		return None;
	}
	Some(packet[1..1 + len].to_vec())
}

pub fn read_varint(buf: &[u8]) -> Result<(usize, usize)> {
	if buf.is_empty() {
		return Err(anyhow!("Buffer empty"));
	}
	let first = buf[0];
	let prefix = first >> 6;
	let len = 1 << prefix;
	if buf.len() < len {
		return Err(anyhow!("Buffer too short for VarInt"));
	}
	let mut val = (first & 0x3f) as u64;
	for i in 1..len {
		val = (val << 8) | (buf[i] as u64);
	}
	Ok((val as usize, len))
}
