/* src/modules/plugins/protocol/tls/clienthello.rs */

use anyhow::Result;

#[derive(Debug, Default)]
pub struct TlsClientHelloData {
	pub legacy_version: String,
	pub random: String,
	pub session_id: String,
	pub cipher_suites: Vec<String>,
	pub compression_methods: Vec<String>,
	pub sni: Option<String>,
	pub alpn: Vec<String>,
	pub supported_versions: Vec<String>,
	pub supported_groups: Vec<String>,
	pub signature_algorithms: Vec<String>,
	pub key_share_groups: Vec<String>,
	pub psk_key_exchange_modes: Vec<String>,
	pub has_renegotiation_info: bool,
	pub has_grease: bool,
}

/// Helper to convert bytes to Hex String
fn to_hex(bytes: &[u8]) -> String {
	hex::encode(bytes)
}

/// Helper to check if a value is GREASE (RFC 8701)
/// Pattern: 0x?A?A where ? is 0-F.
/// Common values: 0A0A, 1A1A, ..., FAFA.
fn is_grease(val: u16) -> bool {
	(val & 0x0F0F) == 0x0A0A
}

/// Helper to read a u16 from slice at cursor
fn read_u16(data: &[u8], cursor: usize) -> Option<u16> {
	if cursor + 2 > data.len() {
		return None;
	}
	Some(((data[cursor] as u16) << 8) | (data[cursor + 1] as u16))
}

/// Main entry point to parse a raw ClientHello buffer.
pub fn parse_client_hello(payload: &[u8]) -> Result<TlsClientHelloData> {
	let mut data = TlsClientHelloData::default();
	let mut cursor = 0;

	// 1. Record Header (5 bytes)
	// Content Type (1) + Version (2) + Length (2)
	// We verify it's a Handshake (0x16) and skip it.
	if payload.len() < 5 {
		return Ok(data);
	}
	if payload[0] != 0x16 {
		return Ok(data);
	}
	cursor += 5;

	// 2. Handshake Header (4 bytes)
	// Type (1) + Length (3)
	// Must be ClientHello (0x01)
	if cursor + 4 > payload.len() {
		return Ok(data);
	}
	if payload[cursor] != 0x01 {
		return Ok(data);
	}
	cursor += 4;

	// 3. Legacy Version (2 bytes)
	if cursor + 2 > payload.len() {
		return Ok(data);
	}
	let ver = read_u16(payload, cursor).unwrap_or(0);
	data.legacy_version = format!("{:04x}", ver);
	cursor += 2;

	// 4. Random (32 bytes)
	if cursor + 32 > payload.len() {
		return Ok(data);
	}
	data.random = to_hex(&payload[cursor..cursor + 32]);
	cursor += 32;

	// 5. Session ID (Variable)
	if cursor + 1 > payload.len() {
		return Ok(data);
	}
	let sess_id_len = payload[cursor] as usize;
	cursor += 1;
	if cursor + sess_id_len > payload.len() {
		return Ok(data);
	}
	if sess_id_len > 0 {
		data.session_id = to_hex(&payload[cursor..cursor + sess_id_len]);
	}
	cursor += sess_id_len;

	// 6. Cipher Suites (Variable)
	if cursor + 2 > payload.len() {
		return Ok(data);
	}
	let cipher_len = read_u16(payload, cursor).unwrap_or(0) as usize;
	cursor += 2;
	if cursor + cipher_len > payload.len() {
		return Ok(data);
	}

	let mut cs_cursor = 0;
	while cs_cursor + 2 <= cipher_len {
		let val = read_u16(&payload[cursor + cs_cursor..], 0).unwrap_or(0);
		if is_grease(val) {
			data.has_grease = true;
		} else {
			data.cipher_suites.push(format!("{:04x}", val));
		}
		cs_cursor += 2;
	}
	cursor += cipher_len;

	// 7. Compression Methods (Variable)
	if cursor + 1 > payload.len() {
		return Ok(data);
	}
	let comp_len = payload[cursor] as usize;
	cursor += 1;
	if cursor + comp_len > payload.len() {
		return Ok(data);
	}
	for i in 0..comp_len {
		data
			.compression_methods
			.push(format!("{:02x}", payload[cursor + i]));
	}
	cursor += comp_len;

	// 8. Extensions (Variable)
	if cursor + 2 > payload.len() {
		return Ok(data);
	}
	let ext_total_len = read_u16(payload, cursor).unwrap_or(0) as usize;
	cursor += 2;

	let end = cursor + ext_total_len;
	if end > payload.len() {
		return Ok(data);
	}

	while cursor + 4 <= end {
		let ext_type = read_u16(payload, cursor).unwrap_or(0);
		let ext_len = read_u16(payload, cursor + 2).unwrap_or(0) as usize;
		cursor += 4;

		if cursor + ext_len > end {
			break;
		}
		let ext_data = &payload[cursor..cursor + ext_len];

		if is_grease(ext_type) {
			data.has_grease = true;
		}

		match ext_type {
			// SNI (0x0000)
			0x0000 => {
				if ext_len >= 5 {
					// ListLen(2) + Type(1) + NameLen(2)
					let name_len = read_u16(ext_data, 3).unwrap_or(0) as usize;
					if 5 + name_len <= ext_len {
						data.sni = Some(String::from_utf8_lossy(&ext_data[5..5 + name_len]).to_string());
					}
				}
			}
			// Supported Groups / Elliptic Curves (0x000a)
			0x000a => {
				if ext_len >= 2 {
					let list_len = read_u16(ext_data, 0).unwrap_or(0) as usize;
					let mut g_cursor = 2;
					while g_cursor + 2 <= 2 + list_len && g_cursor + 2 <= ext_len {
						let group = read_u16(ext_data, g_cursor).unwrap_or(0);
						if is_grease(group) {
							data.has_grease = true;
						} else {
							data.supported_groups.push(format!("{:04x}", group));
						}
						g_cursor += 2;
					}
				}
			}
			// Signature Algorithms (0x000d)
			0x000d => {
				if ext_len >= 2 {
					let list_len = read_u16(ext_data, 0).unwrap_or(0) as usize;
					let mut s_cursor = 2;
					while s_cursor + 2 <= 2 + list_len && s_cursor + 2 <= ext_len {
						let sig = read_u16(ext_data, s_cursor).unwrap_or(0);
						data.signature_algorithms.push(format!("{:04x}", sig));
						s_cursor += 2;
					}
				}
			}
			// ALPN (0x0010)
			0x0010 => {
				if ext_len >= 2 {
					let list_len = read_u16(ext_data, 0).unwrap_or(0) as usize;
					let mut a_cursor = 2;
					while a_cursor < 2 + list_len && a_cursor < ext_len {
						let len = ext_data[a_cursor] as usize;
						a_cursor += 1;
						if a_cursor + len <= ext_len {
							let proto = String::from_utf8_lossy(&ext_data[a_cursor..a_cursor + len]).to_string();
							data.alpn.push(proto);
							a_cursor += len;
						} else {
							break;
						}
					}
				}
			}
			// Supported Versions (0x002b)
			0x002b => {
				if ext_len >= 1 {
					let versions_len = ext_data[0] as usize;
					let mut v_cursor = 1;
					while v_cursor + 2 <= 1 + versions_len && v_cursor + 2 <= ext_len {
						let ver = read_u16(ext_data, v_cursor).unwrap_or(0);
						if is_grease(ver) {
							data.has_grease = true;
						} else {
							data.supported_versions.push(format!("{:04x}", ver));
						}
						v_cursor += 2;
					}
				}
			}
			// PSK Key Exchange Modes (0x002d)
			0x002d => {
				if ext_len >= 1 {
					let modes_len = ext_data[0] as usize;
					for i in 0..modes_len {
						if 1 + i < ext_len {
							data
								.psk_key_exchange_modes
								.push(format!("{:02x}", ext_data[1 + i]));
						}
					}
				}
			}
			// Key Share (0x0033)
			0x0033 => {
				if ext_len >= 2 {
					let client_shares_len = read_u16(ext_data, 0).unwrap_or(0) as usize;
					let mut k_cursor = 2;
					while k_cursor + 4 <= 2 + client_shares_len && k_cursor + 4 <= ext_len {
						let group = read_u16(ext_data, k_cursor).unwrap_or(0);
						let key_exchange_len = read_u16(ext_data, k_cursor + 2).unwrap_or(0) as usize;

						if is_grease(group) {
							data.has_grease = true;
						} else {
							data.key_share_groups.push(format!("{:04x}", group));
						}

						k_cursor += 4 + key_exchange_len;
					}
				}
			}
			// Renegotiation Info (0xff01)
			0xff01 => {
				data.has_renegotiation_info = true;
			}
			_ => {}
		}

		cursor += ext_len;
	}

	Ok(data)
}
