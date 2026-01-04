/* src/plugins/protocol/tls/clienthello.rs */

use anyhow::{Result, anyhow};
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
	let result = parse_tls_plaintext(payload).map_err(|e| anyhow!("TLS parse failed: {:?}", e))?;

	let (_rem, record) = result;

	// We only care about the first record, which should be Handshake
	let msg = record
		.msg
		.first()
		.ok_or_else(|| anyhow!("Empty TLS record"))?;

	let handshake = match msg {
		TlsMessage::Handshake(h) => h,
		_ => return Err(anyhow!("Not a TLS Handshake record")),
	};

	let client_hello = match handshake {
		TlsMessageHandshake::ClientHello(ch) => ch,
		_ => return Err(anyhow!("Not a ClientHello message")),
	};

	let mut data = TlsClientHelloData::default();

	// 1. Basic Fields
	data.legacy_version = format!("{:04x}", client_hello.version.0);
	data.random = hex::encode(client_hello.random);

	if let Some(sid) = client_hello.session_id {
		data.session_id = hex::encode(sid);
	}

	// 2. Cipher Suites
	for cipher in &client_hello.ciphers {
		let val = cipher.0;
		if is_grease(val) {
			data.has_grease = true;
		} else {
			data.cipher_suites.push(format!("{:04x}", val));
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
									data.supported_versions.push(format!("{:04x}", val));
								}
							}
						}
						TlsExtension::EllipticCurves(curves) => {
							for curve in curves {
								let val = curve.0;
								if is_grease(val) {
									data.has_grease = true;
								} else {
									data.supported_groups.push(format!("{:04x}", val));
								}
							}
						}
						TlsExtension::SignatureAlgorithms(sigs) => {
							for sig in sigs {
								data.signature_algorithms.push(format!("{:04x}", sig));
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
									data.key_share_groups.push(format!("{:04x}", group));
								}
							}
						}
						TlsExtension::PskExchangeModes(modes) => {
							for mode in modes {
								data.psk_key_exchange_modes.push(format!("{:02x}", mode));
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
				return Err(anyhow!("Failed to parse TLS extensions: {:?}", e));
			}
		}
	}

	Ok(data)
}
