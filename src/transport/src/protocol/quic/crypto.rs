/* src/transport/src/protocol/quic/crypto.rs */

use super::frame;
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use ring::{aead, hkdf};

use std::collections::BTreeMap;

/// Decrypts payload and extracts SNI and raw Crypto Frames.
pub fn extract_decrypted_content(
	full_packet: &[u8],
	header_start: usize,
	protected_payload_start: usize,
	remaining_len: usize,
	dcid: &[u8],
	version: u32,
) -> Result<(Option<String>, BTreeMap<usize, Vec<u8>>)> {
	if version != 0x00000001 {
		return Err(anyhow!("Unsupported QUIC version"));
	}

	match decrypt_payload(full_packet, header_start, protected_payload_start, remaining_len, dcid) {
		Ok(decrypted) => {
			log(LogLevel::Debug, &format!("✓ Decrypted QUIC payload ({} bytes)", decrypted.len()));
			// Delegate to frame parser
			frame::parse_crypto_frames_for_sni(&decrypted)
		}
		Err(e) => {
			log(LogLevel::Debug, &format!("✗ Failed to decrypt/parse: {e}"));
			Err(e)
		}
	}
}

fn decrypt_payload(
	full_packet: &[u8],
	header_start: usize,
	protected_payload_start: usize,
	remaining_len: usize,
	dcid: &[u8],
) -> Result<Vec<u8>> {
	// RFC 9001: Initial Secrets
	const INITIAL_SALT_V1: &[u8] = &[
		0x38, 0x76, 0x2c, 0xf7, 0xf5, 0x59, 0x34, 0xb3, 0x4d, 0x17, 0x9a, 0xe6, 0xa4, 0xc8, 0x0c, 0xad,
		0xcc, 0xbb, 0x7f, 0x0a,
	];

	let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, INITIAL_SALT_V1);
	let initial_secret = salt.extract(dcid);
	let client_initial_secret_bytes = hkdf_expand_label(&initial_secret, b"client in", &[], 32)?;
	let client_initial_secret =
		hkdf::Prk::new_less_safe(hkdf::HKDF_SHA256, &client_initial_secret_bytes);
	let key_bytes = hkdf_expand_label(&client_initial_secret, b"quic key", &[], 16)?;
	let iv_bytes = hkdf_expand_label(&client_initial_secret, b"quic iv", &[], 12)?;
	let hp_bytes = hkdf_expand_label(&client_initial_secret, b"quic hp", &[], 16)?;

	// Remove Header Protection
	let protected_payload =
		&full_packet[protected_payload_start..protected_payload_start + remaining_len];
	let (pn, pn_len, unprotected_first_byte) =
		remove_header_protection(full_packet[header_start], protected_payload, &hp_bytes)?;

	// Decrypt AEAD
	let mut aad = Vec::new();
	aad.push(unprotected_first_byte);
	aad.extend_from_slice(&full_packet[header_start + 1..protected_payload_start]);
	for i in 0..pn_len {
		aad.push((pn >> (8 * (pn_len - 1 - i))) as u8);
	}

	let mut nonce = [0u8; 12];
	nonce.copy_from_slice(&iv_bytes);
	let pn_offset = 12 - pn_len;
	for i in 0..pn_len {
		nonce[pn_offset + i] ^= (pn >> (8 * (pn_len - 1 - i))) as u8;
	}

	let encrypted_payload = &protected_payload[pn_len..];
	let unbound_key =
		aead::UnboundKey::new(&aead::AES_128_GCM, &key_bytes).map_err(|_| anyhow!("Key error"))?;
	let opening_key = aead::LessSafeKey::new(unbound_key);
	let nonce_obj =
		aead::Nonce::try_assume_unique_for_key(&nonce).map_err(|_| anyhow!("Nonce error"))?;

	let mut decrypted = encrypted_payload.to_vec();
	opening_key
		.open_in_place(nonce_obj, aead::Aad::from(&aad), &mut decrypted)
		.map_err(|e| anyhow!("AEAD error: {e:?}"))?;

	// Remove Tag
	if decrypted.len() < 16 {
		return Err(anyhow!("Decrypted too short"));
	}
	decrypted.truncate(decrypted.len() - 16);

	Ok(decrypted)
}

fn remove_header_protection(first: u8, payload: &[u8], hp_key: &[u8]) -> Result<(u64, usize, u8)> {
	use aes::Aes128;
	use aes::cipher::{BlockEncrypt, KeyInit};

	if payload.len() < 20 {
		return Err(anyhow!("Payload too short for HP"));
	}
	let sample = &payload[4..20];
	let cipher = Aes128::new_from_slice(hp_key).map_err(|_| anyhow!("HP Key error"))?;
	let mut mask_block = aes::Block::clone_from_slice(sample);
	cipher.encrypt_block(&mut mask_block);
	let mask = mask_block.as_slice();

	let unprotected_first = first ^ (mask[0] & 0x0f);
	let pn_len = ((unprotected_first & 0x03) + 1) as usize;
	if pn_len > payload.len() {
		return Err(anyhow!("Truncated PN"));
	}

	let mut pn = 0u64;
	for i in 0..pn_len {
		pn = (pn << 8) | ((payload[i] ^ mask[1 + i]) as u64);
	}

	log(
		LogLevel::Debug,
		&format!("✓ Removed HP: PN={pn} (len={pn_len}), first=0x{unprotected_first:02x}"),
	);
	Ok((pn, pn_len, unprotected_first))
}

fn hkdf_expand_label(
	secret: &ring::hkdf::Prk,
	label: &[u8],
	context: &[u8],
	len: usize,
) -> Result<Vec<u8>> {
	let mut hkdf_label = Vec::new();
	hkdf_label.extend_from_slice(&(len as u16).to_be_bytes());
	let full_label = [b"tls13 ", label].concat();
	hkdf_label.push(full_label.len() as u8);
	hkdf_label.extend_from_slice(&full_label);
	hkdf_label.push(context.len() as u8);
	hkdf_label.extend_from_slice(context);

	let mut out = vec![0u8; len];
	secret
		.expand(&[&hkdf_label], QuicHkdfExpander(len))
		.map_err(|_| anyhow!("HKDF error"))?
		.fill(&mut out)
		.map_err(|_| anyhow!("HKDF fill error"))?;
	Ok(out)
}

struct QuicHkdfExpander(usize);
impl ring::hkdf::KeyType for QuicHkdfExpander {
	fn len(&self) -> usize {
		self.0
	}
}
