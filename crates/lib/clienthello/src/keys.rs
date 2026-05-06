//! RFC 9001 §5.2 Initial Secret derivation for QUIC v1.

use hkdf::Hkdf;
use sha2::Sha256;

use crate::Error;

/// QUIC v1 Initial Salt (RFC 9001 §5.2). Pinned by the RFC; will not
/// change for v1.
pub(crate) const INITIAL_SALT_V1: [u8; 20] = [
	0x38, 0x76, 0x2c, 0xf7, 0xf5, 0x59, 0x34, 0xb3, 0x4d, 0x17, 0x9a, 0xe6, 0xa4, 0xc8, 0x0c, 0xad,
	0xcc, 0xbb, 0x7f, 0x0a,
];

/// Derived client-side Initial keys.
#[derive(Debug, Clone)]
pub(crate) struct InitialKeys {
	/// AES-128-GCM key (16 bytes).
	pub(crate) key: [u8; 16],
	/// AEAD nonce / IV (12 bytes).
	pub(crate) iv: [u8; 12],
	/// Header-protection key for AES-128-ECB (16 bytes).
	pub(crate) hp: [u8; 16],
}

/// Derive client-side Initial keys from the connection's Destination
/// Connection ID, per RFC 9001 §5.2:
///
/// ```text
/// initial_secret = HKDF-Extract(initial_salt, dcid)
/// client_initial_secret = HKDF-Expand-Label(initial_secret, "client in", "", 32)
/// key = HKDF-Expand-Label(client_initial_secret, "quic key", "", 16)
/// iv  = HKDF-Expand-Label(client_initial_secret, "quic iv",  "", 12)
/// hp  = HKDF-Expand-Label(client_initial_secret, "quic hp",  "", 16)
/// ```
pub(crate) fn derive_client_initial_keys(dcid: &[u8]) -> Result<InitialKeys, Error> {
	let hk = Hkdf::<Sha256>::new(Some(&INITIAL_SALT_V1), dcid);
	let mut client_secret = [0u8; 32];
	hkdf_expand_label(&hk, b"client in", &mut client_secret).map_err(|()| Error::HeaderParse)?;

	let client_hk = Hkdf::<Sha256>::from_prk(&client_secret).map_err(|_| Error::HeaderParse)?;
	let mut key = [0u8; 16];
	hkdf_expand_label(&client_hk, b"quic key", &mut key).map_err(|()| Error::HeaderParse)?;
	let mut iv = [0u8; 12];
	hkdf_expand_label(&client_hk, b"quic iv", &mut iv).map_err(|()| Error::HeaderParse)?;
	let mut hp = [0u8; 16];
	hkdf_expand_label(&client_hk, b"quic hp", &mut hp).map_err(|()| Error::HeaderParse)?;

	Ok(InitialKeys { key, iv, hp })
}

/// HKDF-Expand-Label per RFC 8446 §7.1, used by QUIC for the Initial
/// keys (RFC 9001 §5.1):
///
/// ```text
/// HkdfLabel = struct {
///     uint16 length        = okm_len;
///     opaque label<7..255> = "tls13 " + base;   // length-prefixed (1 byte)
///     opaque context<0..255> = "";              // length-prefixed (1 byte), empty
/// };
/// ```
///
/// QUIC v1 only uses SHA-256, so the helper monomorphises against
/// `Hkdf<Sha256>` rather than carrying the full HKDF generic boilerplate.
fn hkdf_expand_label(hk: &Hkdf<Sha256>, label: &[u8], okm: &mut [u8]) -> Result<(), ()> {
	let okm_len = u16::try_from(okm.len()).map_err(|_| ())?;
	let prefix = b"tls13 ";
	let total_label_len = prefix.len() + label.len();
	let label_len = u8::try_from(total_label_len).map_err(|_| ())?;
	let mut info: Vec<u8> = Vec::with_capacity(2 + 1 + total_label_len + 1);
	info.extend_from_slice(&okm_len.to_be_bytes());
	info.push(label_len);
	info.extend_from_slice(prefix);
	info.extend_from_slice(label);
	info.push(0); // context length = 0 (empty context)
	hk.expand(&info, okm).map_err(|_| ())
}

#[cfg(test)]
mod tests {
	use super::*;

	// RFC 9001 Appendix A.1 fixes a known DCID and walks every derived
	// secret/key/iv/hp byte. These tests assert byte-for-byte against
	// the RFC; if the salt, label structure, or HKDF chain ever drifts,
	// the test fails loudly with a known-answer mismatch.
	const RFC_DCID: [u8; 8] = [0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];

	#[test]
	fn initial_salt_matches_rfc_9001() {
		let expected = [
			0x38, 0x76, 0x2c, 0xf7, 0xf5, 0x59, 0x34, 0xb3, 0x4d, 0x17, 0x9a, 0xe6, 0xa4, 0xc8, 0x0c,
			0xad, 0xcc, 0xbb, 0x7f, 0x0a,
		];
		assert_eq!(INITIAL_SALT_V1, expected);
	}

	#[test]
	fn client_initial_keys_match_rfc_9001_appendix_a_1() {
		// RFC 9001 Appendix A.1: with DCID = 0x8394c8f03e515708 the
		// client gets these specific key/iv/hp values.
		let expected_key = hex_to_bytes_16("1f369613dd76d5467730efcbe3b1a22d");
		let expected_iv = hex_to_bytes_12("fa044b2f42a3fd3b46fb255c");
		let expected_hp = hex_to_bytes_16("9f50449e04a0e810283a1e9933adedd2");

		let keys = derive_client_initial_keys(&RFC_DCID).expect("derive");
		assert_eq!(keys.key, expected_key, "client AES key mismatch vs RFC 9001 A.1");
		assert_eq!(keys.iv, expected_iv, "client AEAD IV mismatch vs RFC 9001 A.1");
		assert_eq!(keys.hp, expected_hp, "client header-protection key mismatch vs RFC 9001 A.1");
	}

	fn hex_to_bytes_16(s: &str) -> [u8; 16] {
		let v = decode_hex(s);
		let mut a = [0u8; 16];
		a.copy_from_slice(&v);
		a
	}

	fn hex_to_bytes_12(s: &str) -> [u8; 12] {
		let v = decode_hex(s);
		let mut a = [0u8; 12];
		a.copy_from_slice(&v);
		a
	}

	fn decode_hex(s: &str) -> Vec<u8> {
		assert!(s.len().is_multiple_of(2));
		(0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
	}
}
