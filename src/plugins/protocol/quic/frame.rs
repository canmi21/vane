/* src/plugins/protocol/quic/frame.rs */

use super::packet::read_varint;
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use std::collections::BTreeMap;

/// Parses decrypted payload, extracts ALL crypto frames and attempts SNI extraction.
pub fn parse_crypto_frames_for_sni(
	payload: &[u8],
) -> Result<(Option<String>, BTreeMap<usize, Vec<u8>>)> {
	let mut cursor = 0;
	let mut crypto_map: BTreeMap<usize, Vec<u8>> = BTreeMap::new();

	while cursor < payload.len() {
		let (frame_type, len) = read_varint(&payload[cursor..])?;
		cursor += len;

		match frame_type {
			0x06 => {
				// CRYPTO Frame
				let (offset, off_len) = read_varint(&payload[cursor..])?;
				cursor += off_len;
				let (length, len_len) = read_varint(&payload[cursor..])?;
				cursor += len_len;

				if cursor + length > payload.len() {
					return Err(anyhow!("Truncated CRYPTO frame"));
				}
				let data = payload[cursor..cursor + length].to_vec();

				log(
					LogLevel::Debug,
					&format!("⚙ Found CRYPTO frame: off={}, len={}", offset, length),
				);
				crypto_map.insert(offset, data);

				cursor += length;
			}
			0x00 | 0x01 => {} // PADDING / PING (Ignore)
			0x02 | 0x03 => {
				break;
			} // ACK (Stop parsing)
			_ => {
				break;
			} // Unknown (Stop)
		}
	}

	// Try to reassemble stream starting from 0 to find ClientHello
	let mut stream = Vec::new();
	let mut next = 0;
	for (off, data) in &crypto_map {
		if *off == next {
			stream.extend_from_slice(data);
			next += data.len();
		}
	}

	let sni = if !stream.is_empty() {
		match parse_tls_client_hello_sni(&stream) {
			Ok(s) => Some(s),
			Err(_) => None, // Might be incomplete, that's fine
		}
	} else {
		None
	};

	Ok((sni, crypto_map))
}

pub fn parse_tls_client_hello_sni(data: &[u8]) -> Result<String> {
	let mut cursor = 0;
	if cursor + 4 > data.len() {
		return Err(anyhow!("Truncated TLS header"));
	}
	if data[cursor] != 0x01 {
		return Err(anyhow!("Not ClientHello"));
	}
	cursor += 1;

	let len = u32::from_be_bytes([0, data[cursor], data[cursor + 1], data[cursor + 2]]) as usize;
	cursor += 3;

	let limit = data.len();
	// Check if we have enough data declared
	if cursor + len > limit {
		// Log but try to parse partial
		// log(LogLevel::Debug, &format!("⚠ Partial ClientHello: have {}/{} bytes", limit - cursor, len));
	}

	if cursor + 2 > limit {
		return Err(anyhow!("Truncated ver"));
	}
	cursor += 2;
	if cursor + 32 > limit {
		return Err(anyhow!("Truncated rand"));
	}
	cursor += 32;

	if cursor + 1 > limit {
		return Err(anyhow!("Truncated sess id"));
	}
	let sid_len = data[cursor] as usize;
	cursor += 1 + sid_len;

	if cursor + 2 > limit {
		return Err(anyhow!("Truncated cipher len"));
	}
	let ciph_len = u16::from_be_bytes([data[cursor], data[cursor + 1]]) as usize;
	cursor += 2 + ciph_len;

	if cursor + 1 > limit {
		return Err(anyhow!("Truncated comp len"));
	}
	let comp_len = data[cursor] as usize;
	cursor += 1 + comp_len;

	if cursor + 2 > limit {
		return Err(anyhow!("Truncated ext len"));
	}
	let ext_len = u16::from_be_bytes([data[cursor], data[cursor + 1]]) as usize;
	cursor += 2;

	let end = std::cmp::min(cursor + ext_len, limit);
	while cursor + 4 <= end {
		let etype = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
		let elen = u16::from_be_bytes([data[cursor + 2], data[cursor + 3]]) as usize;
		cursor += 4;

		if cursor + elen > limit {
			return Err(anyhow!("Truncated extension"));
		}
		if etype == 0x0000 {
			return parse_sni_value(&data[cursor..cursor + elen]);
		}
		cursor += elen;
	}

	Err(anyhow!("SNI not found"))
}

fn parse_sni_value(data: &[u8]) -> Result<String> {
	let mut c = 0;
	if c + 2 > data.len() {
		return Err(anyhow!("Truncated list len"));
	}
	c += 2;
	if c + 1 > data.len() {
		return Err(anyhow!("Truncated type"));
	}
	if data[c] != 0x00 {
		return Err(anyhow!("Not host_name"));
	}
	c += 1;
	if c + 2 > data.len() {
		return Err(anyhow!("Truncated name len"));
	}
	let len = u16::from_be_bytes([data[c], data[c + 1]]) as usize;
	c += 2;
	if c + len > data.len() {
		return Err(anyhow!("Truncated name"));
	}
	Ok(String::from_utf8_lossy(&data[c..c + len]).to_string())
}
