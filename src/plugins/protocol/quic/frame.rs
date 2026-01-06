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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_crypto_frames_basic() {
		// Construct a payload with a single CRYPTO frame (type 0x06)
		// Offset: 0, Length: 4, Data: [0xaa, 0xbb, 0xcc, 0xdd]
		let mut payload = vec![0x06, 0x00, 0x04];
		payload.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);

		let (sni, map) = parse_crypto_frames_for_sni(&payload).unwrap();
		assert!(sni.is_none());
		assert_eq!(map.len(), 1);
		assert_eq!(map.get(&0).unwrap(), &vec![0xaa, 0xbb, 0xcc, 0xdd]);
	}

	#[test]
	fn test_parse_multiple_crypto_frames() {
		// Frame 1: Off 0, Len 2 [0x01, 0x02]
		// Frame 2: Off 2, Len 2 [0x03, 0x04]
		let payload = vec![
			0x06, 0x00, 0x02, 0x01, 0x02, // F1
			0x06, 0x02, 0x02, 0x03, 0x04, // F2
		];

		let (_, map) = parse_crypto_frames_for_sni(&payload).unwrap();
		assert_eq!(map.len(), 2);
		assert_eq!(map.get(&0).unwrap(), &vec![0x01, 0x02]);
		assert_eq!(map.get(&2).unwrap(), &vec![0x03, 0x04]);
	}

	#[test]
	fn test_parse_tls_client_hello_sni_extraction() {
		// Construct a minimal TLS 1.3 ClientHello (Handshake only, no Record layer as per QUIC CRYPTO stream)
		// Handshake Header: [Type: 1(CH), Len: 3-bytes]
		let mut ch = vec![0x01, 0x00, 0x00, 0x30]; // Type 1, Len 48
		ch.extend_from_slice(&[0x03, 0x03]); // Ver 1.2
		ch.extend_from_slice(&[0x00; 32]); // Random
		ch.push(0x00); // Session ID len 0
		ch.extend_from_slice(&[0x00, 0x02, 0x00, 0x2f]); // Ciphers (len 2, 1 cipher)
		ch.extend_from_slice(&[0x01, 0x00]); // Comp (len 1, null)

		// Extensions
		let mut ext = vec![0x00, 0x00]; // Type SNI
		let sni_name = b"vane.test";
		let mut sni_val = vec![0x00, 0x00]; // List len (will fill later)
		sni_val.push(0x00); // Type HostName
		sni_val.extend_from_slice(&((sni_name.len() as u16).to_be_bytes())); // Name len
		sni_val.extend_from_slice(sni_name);

		// Fill list len
		let list_len = (sni_val.len() - 2) as u16;
		sni_val[0..2].copy_from_slice(&list_len.to_be_bytes());

		ext.extend_from_slice(&((sni_val.len() as u16).to_be_bytes()));
		ext.extend_from_slice(&sni_val);

		let ext_total_len = ext.len() as u16;
		ch.extend_from_slice(&ext_total_len.to_be_bytes());
		ch.extend_from_slice(&ext);

		// Fix Handshake Length
		let total_handshake_body = (ch.len() - 4) as u32;
		let body_len_bytes = &total_handshake_body.to_be_bytes()[1..4];
		ch[1..4].copy_from_slice(body_len_bytes);

		let sni = parse_tls_client_hello_sni(&ch).unwrap();
		assert_eq!(sni, "vane.test");
	}

	#[test]
	fn test_truncated_frames() {
		// CRYPTO frame header says 10 bytes, but only 5 provided
		let payload = vec![0x06, 0x00, 0x0a, 0x01, 0x02, 0x03, 0x04, 0x05];
		assert!(parse_crypto_frames_for_sni(&payload).is_err());
	}
}
