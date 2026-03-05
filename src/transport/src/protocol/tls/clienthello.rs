/* src/transport/src/protocol/tls/clienthello.rs */

use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use tls_parser::{
	TlsExtension, TlsMessage, TlsMessageHandshake, parse_tls_extensions, parse_tls_plaintext,
};

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

fn is_grease(val: u16) -> bool {
	(val & 0x0F0F) == 0x0A0A
}

/// Main entry point to parse a raw ClientHello buffer using tls-parser.
pub fn parse_client_hello(payload: &[u8]) -> Result<TlsClientHelloData> {
	// Parse the TLS Record Layer
	let result = parse_tls_plaintext(payload).map_err(|e| anyhow!("TLS parse failed: {e:?}"))?;

	let (_rem, record) = result;

	// We only care about the first record, which should be Handshake
	let msg = record.msg.first().ok_or_else(|| anyhow!("Empty TLS record"))?;

	log(LogLevel::Debug, &format!("⚙ TLS Message type: {msg:?}"));

	let TlsMessage::Handshake(handshake) = msg else {
		return Err(anyhow!("Not a TLS Handshake record"));
	};

	let TlsMessageHandshake::ClientHello(client_hello) = handshake else {
		return Err(anyhow!("Not a ClientHello message"));
	};

	let mut data = TlsClientHelloData {
		legacy_version: format!("{:04x}", client_hello.version.0),
		random: hex::encode(client_hello.random),
		..Default::default()
	};

	if let Some(sid) = client_hello.session_id {
		data.session_id = hex::encode(sid);
	}

	// 2. Cipher Suites
	for cipher in &client_hello.ciphers {
		let val = cipher.0;
		if is_grease(val) {
			data.has_grease = true;
		} else {
			data.cipher_suites.push(format!("{val:04x}"));
		}
	}

	// 3. Compression
	for comp in &client_hello.comp {
		data.compression_methods.push(format!("{:02x}", comp.0));
	}

	// 4. Extensions
	if let Some(ext_bytes) = client_hello.ext {
		match parse_tls_extensions(ext_bytes) {
			Ok((_rem, extensions)) => {
				for ext in extensions {
					match ext {
						TlsExtension::SNI(sni_vec) => {
							for (sni_type, sni_name) in sni_vec {
								if sni_type == tls_parser::SNIType::HostName {
									data.sni = Some(String::from_utf8_lossy(sni_name).to_string());
								}
							}
						}
						TlsExtension::ALPN(protos) => {
							for proto in protos {
								data.alpn.push(String::from_utf8_lossy(proto).to_string());
							}
						}
						TlsExtension::SupportedVersions(versions) => {
							for ver in versions {
								let val = ver.0;
								if is_grease(val) {
									data.has_grease = true;
								} else {
									data.supported_versions.push(format!("{val:04x}"));
								}
							}
						}
						TlsExtension::EllipticCurves(curves) => {
							for curve in curves {
								let val = curve.0;
								if is_grease(val) {
									data.has_grease = true;
								} else {
									data.supported_groups.push(format!("{val:04x}"));
								}
							}
						}
						TlsExtension::SignatureAlgorithms(sigs) => {
							for sig in sigs {
								data.signature_algorithms.push(format!("{sig:04x}"));
							}
						}
						TlsExtension::KeyShare(key_share_bytes) => {
							// Manual parsing of KeyShare raw bytes
							let bytes = key_share_bytes;
							let mut offset = 2; // Skip first 2 bytes (total length)

							while offset + 4 <= bytes.len() {
								let group = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
								offset += 2;

								let key_len = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
								offset += 2;

								if offset + key_len > bytes.len() {
									break;
								}
								offset += key_len;

								if is_grease(group) {
									data.has_grease = true;
								} else {
									data.key_share_groups.push(format!("{group:04x}"));
								}
							}
						}
						TlsExtension::PskExchangeModes(modes) => {
							for mode in modes {
								data.psk_key_exchange_modes.push(format!("{mode:02x}"));
							}
						}
						TlsExtension::RenegotiationInfo(_) => {
							data.has_renegotiation_info = true;
						}
						TlsExtension::Grease(val, _) => {
							data.has_grease = true;
							if !is_grease(val) {
								// Shouldn't happen
							}
						}
						TlsExtension::Unknown(type_u16, _) => {
							if is_grease(type_u16.0) {
								data.has_grease = true;
							}
						}
						_ => {}
					}
				}
			}
			Err(e) => {
				return Err(anyhow!("Failed to parse TLS extensions: {e:?}"));
			}
		}
	}

	Ok(data)
}

#[cfg(test)]
mod tests {
	use super::*;
	use fancy_log::set_log_level;

	#[test]
	fn test_is_grease() {
		assert!(is_grease(0x0a0a));
		assert!(is_grease(0x1a1a));
		assert!(is_grease(0x2a2a));
		assert!(is_grease(0x7a7a));
		assert!(!is_grease(0x1234));
		assert!(!is_grease(0x0303));
	}

	#[test]
	fn test_parse_valid_client_hello() {
		set_log_level(LogLevel::Debug);

		// A standard TLS 1.2 ClientHello with SNI: "google"
		let raw_hex = "16030100850100008103031234567812345678123456781234567812345678123456781234567812345678000002002f010000560000000b0009000006676f6f676c65000b000403000102000a000c000a001d0017001e00190018002300000016000000170000000d0020001e060106020603050105020503040104020403030103020303020102020203";
		let payload = hex::decode(raw_hex).unwrap();

		let res = parse_client_hello(&payload);
		assert!(res.is_ok(), "Should parse valid hex: {:?}", res.err());
		let data = res.unwrap();

		assert_eq!(data.sni, Some("google".to_owned()));
		assert_eq!(data.legacy_version, "0303");
	}

	#[test]
	fn test_parse_minimal_client_hello() {
		// Very basic ClientHello with no extensions
		// Content: Handshake(ClientHello), Version 3.3, Random, SessionID(Empty), Ciphers(2), Comp(1), Extensions(0)
		let raw_hex = "160301002d010000290303000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f000002002f0100";
		let payload = hex::decode(raw_hex).unwrap();

		let res = parse_client_hello(&payload).unwrap();
		assert_eq!(res.sni, None);
		assert!(res.alpn.is_empty());
		assert!(!res.has_grease);
	}

	#[test]
	fn test_parse_malformed_data() {
		// Not a TLS record (starts with HTTP GET)
		assert!(parse_client_hello(b"GET / HTTP/1.1").is_err());

		// Truncated record
		assert!(parse_client_hello(&[0x16, 0x03, 0x01, 0x00, 0x05]).is_err());
	}

	#[test]
	fn test_grease_detection() {
		// ClientHello with a GREASE cipher (0x0a0a)
		let raw_hex = "160301002f0100002b0303000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f0000040a0a002f0100";
		let payload = hex::decode(raw_hex).unwrap();

		let res = parse_client_hello(&payload).unwrap();
		assert!(res.has_grease);
		// GREASE should be filtered out from cipher_suites vector
		assert!(!res.cipher_suites.contains(&"0a0a".to_owned()));
	}
}
