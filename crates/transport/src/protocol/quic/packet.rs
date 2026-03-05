/* src/plugins/protocol/quic/packet.rs */

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
		return Err(anyhow!("Not an Initial Packet (Type: {packet_type})"));
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
	let version = format!("0x{version_val:08x}");
	cursor += 4;

	// 5. DCID
	if cursor + 1 > payload.len() {
		return Err(anyhow!("Truncated DCID Length"));
	}
	let dcid_len = payload[cursor] as usize;
	cursor += 1;
	if dcid_len > 20 {
		return Err(anyhow!("DCID length {dcid_len} exceeds 20"));
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
		return Err(anyhow!("SCID length {scid_len} exceeds 20"));
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
#[must_use]
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

#[must_use]
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
	for b in buf.iter().take(len).skip(1) {
		val = (val << 8) | (*b as u64);
	}
	Ok((val as usize, len))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_read_varint() {
		// 1-byte (0 to 63)
		assert_eq!(read_varint(&[0x25]).unwrap(), (37, 1));
		// 2-byte (64 to 16383) -> 0x40 0x00 is 0
		assert_eq!(read_varint(&[0x40, 0x40]).unwrap(), (64, 2));
		assert_eq!(read_varint(&[0x7b, 0xbd]).unwrap(), (15293, 2));
		// 4-byte
		assert_eq!(
			read_varint(&[0x9d, 0x7f, 0x3e, 0x7d]).unwrap(),
			(494878333, 4)
		);
	}

	#[test]
	fn test_peek_long_header_dcid() {
		// Long Header: [First(1)] [Version(4)] [DCIDLen(1)] [DCID(N)]
		// DCIDLen = 8, DCID = 0102030405060708
		let packet = vec![
			0xc0, 0x00, 0x00, 0x00, 0x01, 0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
		];
		let dcid = peek_long_header_dcid(&packet).unwrap();
		assert_eq!(dcid, vec![1, 2, 3, 4, 5, 6, 7, 8]);

		// Too short
		assert!(peek_long_header_dcid(&[0xc0, 0x00]).is_none());
		// DCID length 0 (invalid for Long Header in Vane context typically)
		assert!(peek_long_header_dcid(&[0xc0, 0x00, 0x00, 0x00, 0x01, 0x00]).is_none());
	}

	#[test]
	fn test_peek_short_header_dcid() {
		// Short Header: [First(1)] [DCID(N)]
		let packet = vec![0x40, 0xaa, 0xbb, 0xcc, 0xdd];
		let dcid = peek_short_header_dcid(&packet, 4).unwrap();
		assert_eq!(dcid, vec![0xaa, 0xbb, 0xcc, 0xdd]);

		// Buffer too short for expected DCID length
		assert!(peek_short_header_dcid(&packet, 10).is_none());
	}

	#[test]
	fn test_parse_initial_packet_basic_header() {
		// Minimal Initial Packet structure (without real crypto payload)
		// Byte 0: 11000000 (Initial)
		// Version: 0x00000001
		// DCID: 4 bytes (0x11223344)
		// SCID: 0 bytes
		// Token: 0 (VarInt 0)
		// Length: 0 (VarInt 0)
		let packet = vec![
			0xc0, // Header
			0x00, 0x00, 0x00, 0x01, // Version
			0x04, 0x11, 0x22, 0x33, 0x44, // DCID
			0x00, // SCID Len 0
			0x00, // Token Len 0
			0x00, // Length 0
		];

		// This will likely fail in Step 9 (Crypto) because we have no real payload,
		// but Step 1-8 should pass.
		// Note: extract_decrypted_content returns unwrap_or(...) in current code.
		let res = parse_initial_packet(&packet).unwrap();
		assert_eq!(res.version, "0x00000001");
		assert_eq!(res.dcid, "11223344");
		assert_eq!(res.scid, "");
	}
}
