//! Initial-packet AEAD: header protection (RFC 9001 §5.4) +
//! AES-128-GCM payload decrypt (RFC 9001 §5.3).
//!
//! Header protection mask:
//!
//!   sample = ciphertext[pn_offset + 4 .. pn_offset + 4 + 16]
//!   mask   = AES-128-ECB(hp_key, sample)            // first 5 bytes used
//!   first_byte ^= mask[0] & 0x0f                    // long header low 4 bits
//!   pn_bytes[i] ^= mask[1 + i]                      // 1..=4 bytes
//!
//! AEAD-decrypt:
//!
//!   nonce = iv XOR (8 bytes 0 || u64-be(packet_number))
//!   AAD   = unprotected packet header (first byte through end of PN)
//!   plaintext = AES-128-GCM-decrypt(key, nonce, AAD, ciphertext_payload)

use aes::Aes128;
use aes::cipher::BlockEncrypt;
use aes_gcm::Aes128Gcm;
use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{Aead, KeyInit};

use crate::Error;
use crate::header::InitialHeader;
use crate::keys::InitialKeys;

/// Decrypted Initial packet ready for frame walking.
pub(crate) struct InitialPlaintext {
	pub(crate) payload: Vec<u8>,
}

/// Decrypt a single Initial packet's payload.
///
/// `datagram` may carry trailing bytes belonging to coalesced packets
/// (RFC 9000 §12.2); those are ignored — the parser uses the Length
/// field from `header` to bound the AEAD operation.
pub(crate) fn decrypt_initial(
	datagram: &[u8],
	header: &InitialHeader,
	keys: &InitialKeys,
) -> Result<InitialPlaintext, Error> {
	// Step 1 — gather the 16-byte sample for header protection.
	// RFC 9001 §5.4.2: sample lives at PN offset + 4 (assuming the
	// largest possible PN length of 4 bytes), regardless of the
	// actual encoded PN length.
	let sample_offset = header.packet_number_offset + 4;
	let sample = datagram.get(sample_offset..sample_offset + 16).ok_or(Error::HeaderParse)?;

	// Step 2 — AES-128-ECB to derive the 5-byte mask.
	let cipher = <Aes128 as KeyInit>::new(GenericArray::from_slice(&keys.hp));
	let mut block = [0u8; 16];
	block.copy_from_slice(sample);
	cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
	let mask: [u8; 5] = [block[0], block[1], block[2], block[3], block[4]];

	// Step 3 — recover the unprotected first byte and PN length.
	let protected_first = *datagram.first().ok_or(Error::HeaderParse)?;
	let unprotected_first = protected_first ^ (mask[0] & 0x0f);
	let pn_len = ((unprotected_first & 0x03) as usize) + 1;
	if !(1..=4).contains(&pn_len) {
		return Err(Error::HeaderParse);
	}

	// Step 4 — recover the unprotected packet number bytes.
	let mut pn_bytes = [0u8; 4];
	for i in 0..pn_len {
		let protected = *datagram.get(header.packet_number_offset + i).ok_or(Error::HeaderParse)?;
		pn_bytes[i] = protected ^ mask[1 + i];
	}
	// Decode the truncated packet number into the (possibly truncated)
	// 64-bit value the AEAD uses. For the first Initial of a connection
	// the un-truncation is identity (largest acked = 0).
	let mut packet_number: u64 = 0;
	for byte in pn_bytes.iter().take(pn_len) {
		packet_number = (packet_number << 8) | u64::from(*byte);
	}

	// Step 5 — build the AAD: unprotected header bytes from offset 0
	// through the end of the (now-unprotected) packet number.
	let aad_end = header.packet_number_offset + pn_len;
	let mut aad = Vec::with_capacity(aad_end);
	aad.push(unprotected_first);
	aad.extend_from_slice(&datagram[1..header.packet_number_offset]);
	for byte in pn_bytes.iter().take(pn_len) {
		aad.push(*byte);
	}

	// Step 6 — build the AEAD nonce: iv XOR (zero-padded packet number).
	let mut nonce = keys.iv;
	let pn_be = packet_number.to_be_bytes();
	for i in 0..8 {
		nonce[12 - 8 + i] ^= pn_be[i];
	}

	// Step 7 — AES-128-GCM decrypt of the protected payload.
	let payload_offset = header.packet_number_offset + pn_len;
	let payload_end = header.packet_number_offset + header.packet_length;
	let payload = datagram.get(payload_offset..payload_end).ok_or(Error::HeaderParse)?;

	let aead = <Aes128Gcm as KeyInit>::new(GenericArray::from_slice(&keys.key));
	let plaintext = aead
		.decrypt(GenericArray::from_slice(&nonce), aes_gcm::aead::Payload { msg: payload, aad: &aad })
		.map_err(|_| Error::AeadDecrypt)?;

	Ok(InitialPlaintext { payload: plaintext })
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::header::QUIC_V1;
	use crate::keys::derive_client_initial_keys;

	// Self-consistency round-trip: build a synthetic Initial datagram
	// using known DCID-derived keys and the published encryption
	// algorithm, then run our `decrypt_initial` against it. RFC 9001
	// Appendix A.1 fixes the keys but not a complete encrypted
	// datagram with a chosen plaintext payload, so we exercise the
	// pipeline end-to-end against keys we've already verified
	// byte-for-byte against the RFC (see `keys::tests`).
	#[test]
	fn round_trip_decrypt_recovers_known_plaintext() {
		let dcid = [0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];
		let keys = derive_client_initial_keys(&dcid).expect("keys");
		let plaintext = b"hello-quic-initial-payload";

		// Build datagram: long-header Initial with empty SCID, empty
		// token, our chosen payload. PN length = 1 (encoded in first
		// byte's low 2 bits as 0).
		let mut datagram = Vec::new();
		datagram.push(0xc0); // long-header, fixed bit, type=Initial, PN len=1
		datagram.extend_from_slice(&QUIC_V1.to_be_bytes());
		datagram.push(u8::try_from(dcid.len()).expect("dcid len fits u8"));
		datagram.extend_from_slice(&dcid);
		datagram.push(0); // SCID len = 0
		datagram.push(0); // token VarInt = 0
		// Length VarInt = PN(1) + payload(plaintext.len() + 16-byte tag)
		let length_value = 1 + plaintext.len() + 16;
		// Encode VarInt. For values < 64 the 1-byte form fits;
		// for our test payload (~26 bytes + 17) the value is < 64,
		// so the 1-byte form is fine. Defensive assert keeps that
		// invariant pinned even if the test payload grows.
		assert!(length_value < 64);
		datagram.push(u8::try_from(length_value).expect("length fits u8"));

		let pn_offset = datagram.len();
		datagram.push(0); // PN = 0 (1 byte)

		// Build AAD = current header bytes (PN included).
		let aad = datagram.clone();

		// Encrypt payload under the AEAD with nonce = iv XOR (PN=0).
		let aead = <Aes128Gcm as KeyInit>::new(GenericArray::from_slice(&keys.key));
		let ciphertext = aead
			.encrypt(
				GenericArray::from_slice(&keys.iv),
				aes_gcm::aead::Payload { msg: plaintext, aad: &aad },
			)
			.expect("encrypt");
		datagram.extend_from_slice(&ciphertext);

		// Apply header protection: sample at pn_offset + 4.
		let sample_offset = pn_offset + 4;
		let sample: [u8; 16] =
			datagram[sample_offset..sample_offset + 16].try_into().expect("sample slice");
		let cipher = <Aes128 as KeyInit>::new(GenericArray::from_slice(&keys.hp));
		let mut block = sample;
		cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
		let mask: [u8; 5] = [block[0], block[1], block[2], block[3], block[4]];
		datagram[0] ^= mask[0] & 0x0f;
		datagram[pn_offset] ^= mask[1];

		// Now run our decrypt and assert plaintext recovery.
		let header = InitialHeader::parse(&datagram).expect("header parse");
		let pt = decrypt_initial(&datagram, &header, &keys).expect("decrypt");
		assert_eq!(pt.payload, plaintext);
	}

	#[test]
	fn decrypt_with_wrong_dcid_returns_aead_error() {
		// Build a datagram with one DCID, decrypt with keys from
		// another. The header parses fine (long-header layout is
		// unaffected by DCID identity); decryption fails.
		let dcid_real = [1u8; 8];
		let dcid_wrong = [2u8; 8];
		let keys_real = derive_client_initial_keys(&dcid_real).expect("keys");
		let keys_wrong = derive_client_initial_keys(&dcid_wrong).expect("keys");
		let plaintext = b"nope";

		let mut datagram = Vec::new();
		datagram.push(0xc0);
		datagram.extend_from_slice(&QUIC_V1.to_be_bytes());
		datagram.push(u8::try_from(dcid_real.len()).expect("dcid len fits u8"));
		datagram.extend_from_slice(&dcid_real);
		datagram.push(0);
		datagram.push(0);
		let length_value = 1 + plaintext.len() + 16;
		assert!(length_value < 64);
		datagram.push(u8::try_from(length_value).expect("length fits u8"));
		let pn_offset = datagram.len();
		datagram.push(0);

		let aad = datagram.clone();
		let aead = <Aes128Gcm as KeyInit>::new(GenericArray::from_slice(&keys_real.key));
		let ciphertext = aead
			.encrypt(
				GenericArray::from_slice(&keys_real.iv),
				aes_gcm::aead::Payload { msg: plaintext, aad: &aad },
			)
			.expect("encrypt");
		datagram.extend_from_slice(&ciphertext);

		let sample_offset = pn_offset + 4;
		let sample: [u8; 16] = datagram[sample_offset..sample_offset + 16].try_into().expect("sample");
		let cipher = <Aes128 as KeyInit>::new(GenericArray::from_slice(&keys_real.hp));
		let mut block = sample;
		cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
		let mask = [block[0], block[1], block[2], block[3], block[4]];
		datagram[0] ^= mask[0] & 0x0f;
		datagram[pn_offset] ^= mask[1];

		let header = InitialHeader::parse(&datagram).expect("header parse");
		let result = decrypt_initial(&datagram, &header, &keys_wrong);
		assert!(matches!(result, Err(Error::AeadDecrypt)));
	}
}
