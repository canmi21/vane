/* src/modules/plugins/protocol/quic/parser.rs */

use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use std::collections::BTreeMap;

// FIX: Added Clone to derive macros
#[derive(Debug, Default, Clone)]
pub struct QuicInitialData {
	pub version: String,
	pub dcid: String,
	pub scid: String,
	pub token: Option<String>,
	pub sni_hint: Option<String>,
}

/// Parses a QUIC Long Header Initial Packet and attempts to extract SNI.
///
/// Implements RFC 9000 Section 17.2 (packet structure) and RFC 9001 (Initial Secrets crypto).
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
	let dcid_bytes = &payload[cursor..cursor + dcid_len];
	let dcid = hex::encode(dcid_bytes);
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
		cursor += token_len;
	}

	// 8. Length (VarInt) - length of remaining packet (Packet Number + Payload)
	let (remaining_len, varint_len) = read_varint(&payload[cursor..])?;
	cursor += varint_len;

	if cursor + remaining_len > payload.len() {
		return Err(anyhow!("Truncated packet payload"));
	}

	// Keep header start position for AAD construction
	let header_start = 0;
	let protected_payload_start = cursor;

	// 9. Extract SNI via Initial Secrets decryption (RFC 9001)
	let sni_hint = extract_sni_from_initial(
		payload,
		header_start,
		protected_payload_start,
		remaining_len,
		dcid_bytes,
		version_val,
	);

	Ok(QuicInitialData {
		version,
		dcid,
		scid,
		token,
		sni_hint,
	})
}

/// Extracts SNI from encrypted Initial packet payload using Initial Secrets.
///
/// Process:
/// 1. Derive Initial Secrets from DCID (RFC 9001 Section 5.2)
/// 2. Remove Header Protection to reveal Packet Number
/// 3. Decrypt payload using AEAD
/// 4. Parse CRYPTO frames to extract TLS ClientHello
/// 5. Parse ClientHello to get SNI extension
fn extract_sni_from_initial(
	full_packet: &[u8],
	header_start: usize,
	protected_payload_start: usize,
	remaining_len: usize,
	dcid: &[u8],
	version: u32,
) -> Option<String> {
	// Only support QUIC v1 (0x00000001) for now
	if version != 0x00000001 {
		log(
			LogLevel::Debug,
			&format!(
				"Unsupported QUIC version for SNI extraction: 0x{:08x}",
				version
			),
		);
		return None;
	}

	match try_extract_sni(
		full_packet,
		header_start,
		protected_payload_start,
		remaining_len,
		dcid,
		version, // Fix: Added missing version argument
	) {
		Ok(sni) => {
			log(
				LogLevel::Debug,
				&format!("✓ Successfully extracted SNI from QUIC: {}", sni),
			);
			Some(sni)
		}
		Err(e) => {
			log(
				LogLevel::Debug,
				&format!("✗ Failed to extract SNI from QUIC Initial packet: {}", e),
			);
			None
		}
	}
}

fn try_extract_sni(
	full_packet: &[u8],
	header_start: usize,
	protected_payload_start: usize,
	remaining_len: usize,
	dcid: &[u8],
	_version: u32, // Fix: Renamed to _version to suppress unused warning
) -> Result<String> {
	use ring::aead;
	use ring::hkdf;

	// RFC 9001 Section 5.2: Initial Secrets derivation
	const INITIAL_SALT_V1: &[u8] = &[
		0x38, 0x76, 0x2c, 0xf7, 0xf5, 0x59, 0x34, 0xb3, 0x4d, 0x17, 0x9a, 0xe6, 0xa4, 0xc8, 0x0c, 0xad,
		0xcc, 0xbb, 0x7f, 0x0a,
	];

	let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, INITIAL_SALT_V1);
	let initial_secret = salt.extract(dcid);

	// Derive client_initial_secret
	let client_initial_secret_bytes = hkdf_expand_label(
		&initial_secret,
		b"client in",
		&[],
		32, // SHA256 output size
	)?;

	// Convert bytes to a Prk so it can be used as a secret for further derivation
	let client_initial_secret =
		hkdf::Prk::new_less_safe(hkdf::HKDF_SHA256, &client_initial_secret_bytes);

	// Derive key, iv, and hp from client_initial_secret
	let key_bytes = hkdf_expand_label(&client_initial_secret, b"quic key", &[], 16)?;
	let iv_bytes = hkdf_expand_label(&client_initial_secret, b"quic iv", &[], 12)?;
	let hp_bytes = hkdf_expand_label(&client_initial_secret, b"quic hp", &[], 16)?;

	// Remove Header Protection (RFC 9001 Section 5.4)
	let protected_payload =
		&full_packet[protected_payload_start..protected_payload_start + remaining_len];

	let (pn, pn_len, unprotected_first_byte) =
		remove_header_protection(full_packet[header_start], protected_payload, &hp_bytes)?;

	// Construct AAD: unprotected header from start to end of packet number
	let mut aad = Vec::new();
	aad.push(unprotected_first_byte);
	aad.extend_from_slice(&full_packet[header_start + 1..protected_payload_start]);

	// Add unprotected Packet Number bytes
	for i in 0..pn_len {
		aad.push((pn >> (8 * (pn_len - 1 - i))) as u8);
	}

	// Construct nonce: iv XOR packet_number (right-aligned)
	let mut nonce = [0u8; 12];
	nonce.copy_from_slice(&iv_bytes);
	let pn_offset_in_nonce = 12 - pn_len;
	for i in 0..pn_len {
		nonce[pn_offset_in_nonce + i] ^= (pn >> (8 * (pn_len - 1 - i))) as u8;
	}

	// Extract encrypted payload (after Packet Number)
	let encrypted_payload = &protected_payload[pn_len..];

	// Decrypt using AES-128-GCM (QUIC v1 default)
	let unbound_key = aead::UnboundKey::new(&aead::AES_128_GCM, &key_bytes)
		.map_err(|_| anyhow!("Failed to create decryption key"))?;
	let opening_key = aead::LessSafeKey::new(unbound_key);

	let nonce_obj =
		aead::Nonce::try_assume_unique_for_key(&nonce).map_err(|_| anyhow!("Invalid nonce"))?;

	let mut decrypted = encrypted_payload.to_vec();

	// Open in place: the tag is appended at the end (last 16 bytes for GCM)
	opening_key
		.open_in_place(nonce_obj, aead::Aad::from(&aad), &mut decrypted)
		.map_err(|e| anyhow!("AEAD decryption failed: {:?}", e))?;

	// Remove the authentication tag (last 16 bytes)
	if decrypted.len() < 16 {
		return Err(anyhow!("Decrypted payload too short"));
	}
	decrypted.truncate(decrypted.len() - 16);

	log(
		LogLevel::Debug,
		&format!(
			"✓ Successfully decrypted QUIC payload ({} bytes)",
			decrypted.len()
		),
	);

	// Parse CRYPTO frames to extract TLS ClientHello
	parse_crypto_frames_for_sni(&decrypted)
}

/// Removes Header Protection from QUIC packet.
///
/// RFC 9001 Section 5.4: Header Protection uses AES-ECB with the hp_key.
/// Returns: (packet_number, pn_length, unprotected_first_byte)
fn remove_header_protection(
	first_byte: u8,
	protected_payload: &[u8],
	hp_key: &[u8],
) -> Result<(u64, usize, u8)> {
	use aes::Aes128;
	use aes::cipher::{BlockEncrypt, KeyInit};

	// RFC 9001 Section 5.4.2: Sample starts at pn_offset + 4
	// For Initial packets, PN can be 1-4 bytes. We need at least 4 + 16 bytes for sample.
	if protected_payload.len() < 20 {
		return Err(anyhow!(
			"Payload too short for header protection removal (need 20 bytes minimum)"
		));
	}

	// Sample starts 4 bytes into the protected payload
	// (assuming worst case: 4-byte PN, sample starts immediately after)
	let sample = &protected_payload[4..20];

	// Encrypt sample with HP key using AES-ECB to get mask
	let cipher = Aes128::new_from_slice(hp_key).map_err(|_| anyhow!("Invalid HP key length"))?;

	let mut mask_block = aes::Block::clone_from_slice(sample);
	cipher.encrypt_block(&mut mask_block);
	let mask = mask_block.as_slice();

	// Unmask first byte to reveal PN length (bits 1-0 for Long Header)
	let unprotected_first_byte = first_byte ^ (mask[0] & 0x0f);
	let pn_len = ((unprotected_first_byte & 0x03) + 1) as usize;

	if pn_len > protected_payload.len() {
		return Err(anyhow!(
			"Truncated packet number (pn_len={}, payload={})",
			pn_len,
			protected_payload.len()
		));
	}

	// Unmask PN bytes (they are at the start of protected_payload)
	let mut pn = 0u64;
	for i in 0..pn_len {
		let unmasked_byte = protected_payload[i] ^ mask[1 + i];
		pn = (pn << 8) | (unmasked_byte as u64);
	}

	log(
		LogLevel::Debug,
		&format!(
			"✓ Removed HP: PN={} (len={}), first_byte=0x{:02x}",
			pn, pn_len, unprotected_first_byte
		),
	);

	Ok((pn, pn_len, unprotected_first_byte))
}

/// Parses CRYPTO frames from decrypted QUIC payload to extract SNI.
///
/// RFC 9000 Section 19.6: CRYPTO Frame Format
/// Handles fragmented CRYPTO frames by collecting and reassembling them.
fn parse_crypto_frames_for_sni(payload: &[u8]) -> Result<String> {
	let mut cursor = 0;
	let mut crypto_fragments: BTreeMap<usize, Vec<u8>> = BTreeMap::new();

	// Collect all CRYPTO frames
	while cursor < payload.len() {
		// Frame type is a VarInt
		let (frame_type, varint_len) = read_varint(&payload[cursor..])?;
		cursor += varint_len;

		match frame_type {
			0x06 => {
				// CRYPTO frame
				// Offset (VarInt)
				let (offset, varint_len) = read_varint(&payload[cursor..])?;
				cursor += varint_len;

				// Length (VarInt)
				let (length, varint_len) = read_varint(&payload[cursor..])?;
				cursor += varint_len;

				if cursor + length > payload.len() {
					return Err(anyhow!("Truncated CRYPTO frame data"));
				}

				let crypto_data = &payload[cursor..cursor + length];

				log(
					LogLevel::Debug,
					&format!("⚙ Found CRYPTO frame: offset={}, length={}", offset, length),
				);

				// Store fragment
				crypto_fragments.insert(offset, crypto_data.to_vec());

				cursor += length;
			}
			0x00 => {
				// PADDING frame - just one byte, continue
				continue;
			}
			0x01 => {
				// PING frame - no additional data
				continue;
			}
			0x02 | 0x03 => {
				// ACK frames - skip detailed parsing, but try to continue
				log(LogLevel::Debug, "Skipping ACK frame in Initial packet");
				// For simplicity, abort here since we need proper ACK parsing to skip it
				break;
			}
			_ => {
				// Unknown or unhandled frame type
				log(
					LogLevel::Debug,
					&format!("Skipping unknown frame type: 0x{:x}", frame_type),
				);
				// Can't skip safely without parsing length, abort
				break;
			}
		}
	}

	// Reassemble contiguous CRYPTO data starting from offset 0
	let mut reassembled = Vec::new();
	let mut expected_offset = 0;

	for (offset, data) in &crypto_fragments {
		if *offset == expected_offset {
			reassembled.extend_from_slice(data);
			expected_offset += data.len();
		} else if *offset > expected_offset {
			// Gap in data, we can't fully reassemble, but let's see what we have.
			// In partial parsing logic, maybe we already have enough for SNI.
			log(
				LogLevel::Debug,
				&format!(
					"⚠ Gap in CRYPTO data: expected offset {}, got {}. Parsing partial stream.",
					expected_offset, offset
				),
			);
			break;
		}
		// If *offset < expected_offset, it's duplicate/overlapping data, ignore
	}

	if reassembled.is_empty() {
		return Err(anyhow!("No CRYPTO frame data starting from offset 0"));
	}

	log(
		LogLevel::Debug,
		&format!(
			"✓ Reassembled {} bytes of CRYPTO data from {} fragments",
			reassembled.len(),
			crypto_fragments.len()
		),
	);

	// Parse TLS ClientHello from reassembled data
	parse_tls_client_hello_sni(&reassembled)
}

/// Parses TLS ClientHello from CRYPTO frame data to extract SNI.
///
/// Minimal TLS 1.3 ClientHello parser focused on SNI extraction only.
/// Supports partial (fragmented) ClientHellos by parsing as much as possible.
fn parse_tls_client_hello_sni(data: &[u8]) -> Result<String> {
	let mut cursor = 0;
	let available_len = data.len();

	// TLS Handshake Header
	if cursor + 4 > available_len {
		return Err(anyhow!("Truncated TLS handshake header"));
	}

	let msg_type = data[cursor];
	if msg_type != 0x01 {
		// ClientHello
		return Err(anyhow!(
			"Not a ClientHello message (type: 0x{:02x})",
			msg_type
		));
	}
	cursor += 1;

	// Length (3 bytes, big-endian)
	let declared_length =
		u32::from_be_bytes([0, data[cursor], data[cursor + 1], data[cursor + 2]]) as usize;
	cursor += 3;

	// FIX: Best-effort parsing. If we don't have the full body, we check if we have enough
	// to potentially extract SNI (extensions usually appear early).
	if cursor + declared_length > available_len {
		log(
			LogLevel::Debug,
			&format!(
				"⚠ ClientHello declares length {} but only {} bytes available. Attempting partial parse.",
				declared_length,
				available_len - cursor
			),
		);
	}

	// ClientHello content
	// Version (2 bytes)
	if cursor + 2 > available_len {
		return Err(anyhow!("Truncated version field"));
	}
	cursor += 2;

	// Random (32 bytes)
	if cursor + 32 > available_len {
		return Err(anyhow!("Truncated random field"));
	}
	cursor += 32;

	// Session ID Length (1 byte) + Session ID
	if cursor + 1 > available_len {
		return Err(anyhow!("Truncated Session ID length"));
	}
	let session_id_len = data[cursor] as usize;
	cursor += 1;
	if cursor + session_id_len > available_len {
		return Err(anyhow!("Truncated Session ID"));
	}
	cursor += session_id_len;

	// Cipher Suites Length (2 bytes) + Cipher Suites
	if cursor + 2 > available_len {
		return Err(anyhow!("Truncated Cipher Suites length"));
	}
	let cipher_suites_len = u16::from_be_bytes([data[cursor], data[cursor + 1]]) as usize;
	cursor += 2;
	if cursor + cipher_suites_len > available_len {
		return Err(anyhow!("Truncated Cipher Suites"));
	}
	cursor += cipher_suites_len;

	// Compression Methods Length (1 byte) + Compression Methods
	if cursor + 1 > available_len {
		return Err(anyhow!("Truncated Compression Methods length"));
	}
	let compression_len = data[cursor] as usize;
	cursor += 1;
	if cursor + compression_len > available_len {
		return Err(anyhow!("Truncated Compression Methods"));
	}
	cursor += compression_len;

	// Extensions Length (2 bytes)
	if cursor + 2 > available_len {
		// We might not have reached extensions yet if the packet ended early
		return Err(anyhow!("No extensions in partial ClientHello (truncated)"));
	}
	let extensions_len = u16::from_be_bytes([data[cursor], data[cursor + 1]]) as usize;
	cursor += 2;

	// Use the smaller of: available buffer end OR declared extensions end
	let extensions_end = std::cmp::min(cursor + extensions_len, available_len);

	// Parse extensions to find SNI (type 0x0000)
	while cursor + 4 <= extensions_end {
		let ext_type = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
		let ext_len = u16::from_be_bytes([data[cursor + 2], data[cursor + 3]]) as usize;
		cursor += 4;

		// Check if we have the full extension data
		if cursor + ext_len > available_len {
			// We ran out of data in the middle of extensions, and haven't found SNI yet.
			return Err(anyhow!("Truncated extension data (ran out of buffer)"));
		}

		if ext_type == 0x0000 {
			// SNI extension found
			return parse_sni_extension(&data[cursor..cursor + ext_len]);
		}

		cursor += ext_len;
	}

	Err(anyhow!("SNI extension not found (parsed {} bytes)", cursor))
}

/// Parses SNI extension data.
fn parse_sni_extension(data: &[u8]) -> Result<String> {
	let mut cursor = 0;

	// Server Name List Length (2 bytes)
	if cursor + 2 > data.len() {
		return Err(anyhow!("Truncated SNI list length"));
	}
	let _list_len = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
	cursor += 2;

	// Server Name Type (1 byte, 0x00 = host_name)
	if cursor + 1 > data.len() {
		return Err(anyhow!("Truncated SNI type"));
	}
	let name_type = data[cursor];
	cursor += 1;

	if name_type != 0x00 {
		return Err(anyhow!("Unsupported SNI type: {}", name_type));
	}

	// Server Name Length (2 bytes)
	if cursor + 2 > data.len() {
		return Err(anyhow!("Truncated SNI length"));
	}
	let name_len = u16::from_be_bytes([data[cursor], data[cursor + 1]]) as usize;
	cursor += 2;

	if cursor + name_len > data.len() {
		return Err(anyhow!("Truncated SNI value"));
	}

	let sni = std::str::from_utf8(&data[cursor..cursor + name_len])
		.map_err(|_| anyhow!("Invalid UTF-8 in SNI"))?
		.to_string();

	Ok(sni)
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

	Ok((val as usize, len))
}

/// Helper function for HKDF-Expand-Label (RFC 8446 Section 7.1, adapted for QUIC)
fn hkdf_expand_label(
	secret: &ring::hkdf::Prk,
	label: &[u8],
	context: &[u8],
	length: usize,
) -> Result<Vec<u8>> {
	// HkdfLabel structure:
	// struct {
	//     uint16 length = Length;
	//     opaque label<7..255> = "tls13 " + Label;
	//     opaque context<0..255> = Context;
	// } HkdfLabel;

	let mut hkdf_label = Vec::new();
	hkdf_label.extend_from_slice(&(length as u16).to_be_bytes());

	let full_label = [b"tls13 ", label].concat();
	hkdf_label.push(full_label.len() as u8);
	hkdf_label.extend_from_slice(&full_label);

	hkdf_label.push(context.len() as u8);
	hkdf_label.extend_from_slice(context);

	let mut output = vec![0u8; length];

	// ring's expand expects a slice of info slices: &[&[u8]]
	secret
		.expand(&[&hkdf_label], QuicHkdfExpander(length))
		.map_err(|_| anyhow!("HKDF expand failed"))?
		.fill(&mut output)
		.map_err(|_| anyhow!("HKDF fill failed"))?;

	Ok(output)
}

struct QuicHkdfExpander(usize);

impl ring::hkdf::KeyType for QuicHkdfExpander {
	fn len(&self) -> usize {
		self.0
	}
}

/// Lightweight peek for DCID from Long Header packets.
/// Does not perform validation, just extraction.
pub fn peek_long_header_dcid(packet: &[u8]) -> Option<Vec<u8>> {
	// Min size: Flags(1) + Ver(4) + DCID_Len(1)
	if packet.len() < 6 {
		return None;
	}

	// Byte 5 is DCID Length
	let dcid_len = packet[5] as usize;
	if dcid_len == 0 || dcid_len > 20 {
		return None;
	}

	// Check bounds: Header + DCID
	if packet.len() < 6 + dcid_len {
		return None;
	}

	Some(packet[6..6 + dcid_len].to_vec())
}

/// Lightweight peek for DCID from Short Header packets.
/// Since Short Headers don't have length, we try a specific length.
pub fn peek_short_header_dcid(packet: &[u8], len: usize) -> Option<Vec<u8>> {
	// Short Header: Flags(1) + DCID(len)
	if packet.len() < 1 + len {
		return None;
	}
	Some(packet[1..1 + len].to_vec())
}
