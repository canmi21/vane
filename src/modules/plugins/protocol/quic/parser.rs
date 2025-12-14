/* src/modules/plugins/protocol/quic/parser.rs */

use anyhow::{Result, anyhow};

#[derive(Debug, Default)]
pub struct QuicInitialData {
	pub version: String,
	pub dcid: String,
	pub scid: String,
	pub token: Option<String>,
	// Hints are populated if decryption logic (RFC 9001) is added later.
	pub sni_hint: Option<String>,
	pub alpn_hint: Option<String>,
}

/// Parses a QUIC Long Header Initial Packet.
///
/// Implements RFC 9000 Section 17.2 strict parsing logic.
/// Since 'quinn-proto' hides its packet parsing module internally,
/// we maintain this lightweight, standard-compliant implementation.
pub fn parse_initial_packet(payload: &[u8]) -> Result<QuicInitialData> {
	// Minimal size for an Initial packet header:
	// Flags(1) + Version(4) + DCID_Len(1) + SCID_Len(1) + Token_Len(1) + Length(1) = ~9 bytes minimum
	// Realistically, CIDs are rarely empty, so we check a safe lower bound.
	if payload.len() < 10 {
		return Err(anyhow!("Buffer too short for QUIC Long Header"));
	}

	let first_byte = payload[0];

	// 1. Header Form Check (bit 7 must be 1 for Long Header)
	// Reference: RFC 9000 Section 17.2
	if (first_byte & 0x80) == 0 {
		return Err(anyhow!("Not a Long Header packet (Short header?)"));
	}

	// 2. Fixed Bit Check (bit 6 must be 1)
	if (first_byte & 0x40) == 0 {
		return Err(anyhow!("Fixed bit not set in QUIC header"));
	}

	// 3. Packet Type Check (bits 5-4). Initial Packet is 0x00.
	let packet_type = (first_byte & 0x30) >> 4;
	if packet_type != 0 {
		return Err(anyhow!("Not an Initial Packet (Type: {})", packet_type));
	}

	let mut cursor = 1;

	// 4. Version (4 bytes)
	if cursor + 4 > payload.len() {
		return Err(anyhow!("Truncated Version field"));
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

	// 5. Destination Connection ID (DCID)
	// RFC 9000: DCID Length is the byte following Version.
	if cursor + 1 > payload.len() {
		return Err(anyhow!("Truncated DCID Length"));
	}
	let dcid_len = payload[cursor] as usize;
	cursor += 1;

	if dcid_len > 20 {
		// RFC 9000 Section 17.2: CID lengths must not exceed 20 bytes in Initial packets
		return Err(anyhow!("DCID length {} exceeds max of 20", dcid_len));
	}
	if cursor + dcid_len > payload.len() {
		return Err(anyhow!("Truncated DCID"));
	}
	let dcid = hex::encode(&payload[cursor..cursor + dcid_len]);
	cursor += dcid_len;

	// 6. Source Connection ID (SCID)
	if cursor + 1 > payload.len() {
		return Err(anyhow!("Truncated SCID Length"));
	}
	let scid_len = payload[cursor] as usize;
	cursor += 1;

	if scid_len > 20 {
		return Err(anyhow!("SCID length {} exceeds max of 20", scid_len));
	}
	if cursor + scid_len > payload.len() {
		return Err(anyhow!("Truncated SCID"));
	}
	let scid = hex::encode(&payload[cursor..cursor + scid_len]);
	cursor += scid_len;

	// 7. Token (VarInt Length + Bytes)
	// RFC 9000: Initial packets contain a token field.
	let (token_len, varint_len) = read_varint(&payload[cursor..])?;
	cursor += varint_len;

	let mut token = None;
	if token_len > 0 {
		if cursor + token_len > payload.len() {
			return Err(anyhow!("Truncated Token"));
		}
		token = Some(hex::encode(&payload[cursor..cursor + token_len]));
		// cursor += token_len; // Safe to advance if we needed to read Length field next
	}

	// Note: We stop here.
	// The next field is 'Length' (VarInt), followed by Packet Number (protected) and Payload (encrypted).
	// Without implementing the RFC 9001 Crypto (HP removal + Decryption), we cannot proceed further.

	Ok(QuicInitialData {
		version,
		dcid,
		scid,
		token,
		sni_hint: None,
		alpn_hint: None,
	})
}

/// Reads a QUIC Variable-Length Integer (VarInt) from the buffer.
/// Reference: RFC 9000 Section 16.
/// Returns (value, bytes_consumed).
fn read_varint(buf: &[u8]) -> Result<(usize, usize)> {
	if buf.is_empty() {
		return Err(anyhow!("Buffer empty, cannot read VarInt"));
	}

	// The first 2 bits determine the length:
	// 00 -> 1 byte  (6 bits usable)
	// 01 -> 2 bytes (14 bits usable)
	// 10 -> 4 bytes (30 bits usable)
	// 11 -> 8 bytes (62 bits usable)

	let first = buf[0];
	let prefix = first >> 6;
	let len = 1 << prefix; // 1, 2, 4, or 8 bytes total

	if buf.len() < len {
		return Err(anyhow!("Buffer too short for VarInt of length {}", len));
	}

	// Mask out the prefix bits from the first byte to get the most significant bits
	let mut val = (first & 0x3f) as u64;

	for i in 1..len {
		val = (val << 8) | (buf[i] as u64);
	}

	// In Rust, casting u64 to usize is generally safe on 64-bit targets.
	// On 32-bit targets, QUIC allows values up to 2^62-1, which might overflow usize.
	// Vane targets server environments (usually 64-bit), but we can add a saturating check if paranoid.
	Ok((val as usize, len))
}
