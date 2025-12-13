/* src/modules/plugins/protocol/tls/clienthello.rs */

use anyhow::Result;

/// Extracts the SNI from a raw ClientHello bytes.
pub fn extract_sni(payload: &[u8]) -> Result<Option<String>> {
	let mut cursor = 0;

	// Skip Record(5) + Handshake(4) + Version(2) + Random(32) = 43
	if payload.len() < 43 {
		return Ok(None);
	}
	cursor += 43;

	// Session ID
	if cursor + 1 > payload.len() {
		return Ok(None);
	}
	let sess_id_len = payload[cursor] as usize;
	cursor += 1 + sess_id_len;

	// Cipher Suites
	if cursor + 2 > payload.len() {
		return Ok(None);
	}
	let cipher_len = ((payload[cursor] as usize) << 8) | (payload[cursor + 1] as usize);
	cursor += 2 + cipher_len;

	// Compression
	if cursor + 1 > payload.len() {
		return Ok(None);
	}
	let comp_len = payload[cursor] as usize;
	cursor += 1 + comp_len;

	// Extensions
	if cursor + 2 > payload.len() {
		return Ok(None);
	}
	let ext_total_len = ((payload[cursor] as usize) << 8) | (payload[cursor + 1] as usize);
	cursor += 2;

	let end = cursor + ext_total_len;
	if end > payload.len() {
		return Ok(None);
	}

	while cursor + 4 <= end {
		let ext_type = ((payload[cursor] as u16) << 8) | (payload[cursor + 1] as u16);
		let ext_len = ((payload[cursor + 2] as usize) << 8) | (payload[cursor + 3] as usize);
		cursor += 4;

		if cursor + ext_len > end {
			break;
		}

		// SNI Extension (0x0000)
		if ext_type == 0x0000 {
			let list_len = ((payload[cursor] as usize) << 8) | (payload[cursor + 1] as usize);
			if list_len < 3 {
				return Ok(None);
			}

			let sni_type = payload[cursor + 2];
			if sni_type == 0x00 {
				// HostName
				let name_len = ((payload[cursor + 3] as usize) << 8) | (payload[cursor + 4] as usize);
				if cursor + 5 + name_len > end {
					return Ok(None);
				}

				let name_bytes = &payload[cursor + 5..cursor + 5 + name_len];
				return Ok(Some(String::from_utf8_lossy(name_bytes).to_string()));
			}
		}
		cursor += ext_len;
	}

	Ok(None)
}

/// Extracts ALPN protocols from raw ClientHello bytes.
pub fn extract_alpn(payload: &[u8]) -> Result<Vec<String>> {
	let mut cursor = 0;
	let mut protocols = Vec::new();

	// Skip Header (43)
	if payload.len() < 43 {
		return Ok(protocols);
	}
	cursor += 43;

	// Session ID
	if cursor + 1 > payload.len() {
		return Ok(protocols);
	}
	cursor += 1 + (payload[cursor] as usize);

	// Cipher Suites
	if cursor + 2 > payload.len() {
		return Ok(protocols);
	}
	cursor += 2 + (((payload[cursor] as usize) << 8) | (payload[cursor + 1] as usize));

	// Compression
	if cursor + 1 > payload.len() {
		return Ok(protocols);
	}
	cursor += 1 + (payload[cursor] as usize);

	// Extensions
	if cursor + 2 > payload.len() {
		return Ok(protocols);
	}
	let ext_total_len = ((payload[cursor] as usize) << 8) | (payload[cursor + 1] as usize);
	cursor += 2;

	let end = cursor + ext_total_len;
	if end > payload.len() {
		return Ok(protocols);
	}

	while cursor + 4 <= end {
		let ext_type = ((payload[cursor] as u16) << 8) | (payload[cursor + 1] as u16);
		let ext_len = ((payload[cursor + 2] as usize) << 8) | (payload[cursor + 3] as usize);
		cursor += 4;

		if cursor + ext_len > end {
			break;
		}

		// ALPN Extension (0x0010)
		if ext_type == 0x0010 {
			let list_len = ((payload[cursor] as usize) << 8) | (payload[cursor + 1] as usize);
			let mut p_cursor = cursor + 2;
			let p_end = p_cursor + list_len;

			while p_cursor + 1 <= p_end {
				let len = payload[p_cursor] as usize;
				p_cursor += 1;
				if p_cursor + len <= p_end {
					let bytes = &payload[p_cursor..p_cursor + len];
					protocols.push(String::from_utf8_lossy(bytes).to_string());
					p_cursor += len;
				} else {
					break;
				}
			}
			return Ok(protocols);
		}
		cursor += ext_len;
	}

	Ok(protocols)
}
