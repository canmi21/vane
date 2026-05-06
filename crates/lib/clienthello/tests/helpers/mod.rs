//! Test-fixture helpers: build a synthetic QUIC v1 Initial datagram
//! whose CRYPTO frame carries a minimal `ClientHello` with a chosen
//! SNI host name, AEAD-encrypted under keys derived from the given
//! DCID.
//!
//! The encryption side is implemented here against RFC 9001 §5.2/§5.3
//! using the same RustCrypto primitives the crate's decrypt side uses.
//! These helpers are test-only and not exposed in the public API.

#![allow(clippy::doc_markdown)]
#![allow(clippy::similar_names)]
#![allow(clippy::missing_panics_doc)]

use aes::Aes128;
use aes::cipher::BlockEncrypt;
use aes_gcm::Aes128Gcm;
use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{Aead, KeyInit};
use hkdf::Hkdf;
use sha2::Sha256;

const INITIAL_SALT_V1: [u8; 20] = [
	0x38, 0x76, 0x2c, 0xf7, 0xf5, 0x59, 0x34, 0xb3, 0x4d, 0x17, 0x9a, 0xe6, 0xa4, 0xc8, 0x0c, 0xad,
	0xcc, 0xbb, 0x7f, 0x0a,
];

const QUIC_V1: u32 = 0x0000_0001;

#[derive(Clone)]
struct InitialKeys {
	key: [u8; 16],
	iv: [u8; 12],
	hp: [u8; 16],
}

fn hkdf_expand_label(hk: &Hkdf<Sha256>, label: &[u8], okm: &mut [u8]) {
	let okm_len = u16::try_from(okm.len()).expect("okm len");
	let prefix = b"tls13 ";
	let total = prefix.len() + label.len();
	let total_u8 = u8::try_from(total).expect("label len");
	let mut info: Vec<u8> = Vec::new();
	info.extend_from_slice(&okm_len.to_be_bytes());
	info.push(total_u8);
	info.extend_from_slice(prefix);
	info.extend_from_slice(label);
	info.push(0);
	hk.expand(&info, okm).expect("hkdf-expand");
}

fn derive_keys(dcid: &[u8]) -> InitialKeys {
	let hk = Hkdf::<Sha256>::new(Some(&INITIAL_SALT_V1), dcid);
	let mut client_secret = [0u8; 32];
	hkdf_expand_label(&hk, b"client in", &mut client_secret);
	let chk = Hkdf::<Sha256>::from_prk(&client_secret).expect("from_prk");
	let mut key = [0u8; 16];
	let mut iv = [0u8; 12];
	let mut hp = [0u8; 16];
	hkdf_expand_label(&chk, b"quic key", &mut key);
	hkdf_expand_label(&chk, b"quic iv", &mut iv);
	hkdf_expand_label(&chk, b"quic hp", &mut hp);
	InitialKeys { key, iv, hp }
}

fn build_client_hello(sni: &str) -> Vec<u8> {
	let mut body: Vec<u8> = Vec::new();
	body.extend_from_slice(&[0x03, 0x03]); // legacy_version
	body.extend_from_slice(&[0u8; 32]); // random
	body.push(0); // session_id length
	body.extend_from_slice(&2u16.to_be_bytes()); // cipher_suites length
	body.extend_from_slice(&[0x13, 0x01]); // TLS_AES_128_GCM_SHA256
	body.push(1); // compression length
	body.push(0); // null compression

	let mut ext: Vec<u8> = Vec::new();
	let sni_bytes = sni.as_bytes();
	let host_name_len = u16::try_from(sni_bytes.len()).expect("sni len");
	let server_name_entry_len = 1 + 2 + host_name_len;
	let list_len = server_name_entry_len;
	let ext_payload_len = 2 + list_len;
	ext.extend_from_slice(&0x0000_u16.to_be_bytes()); // extension type = server_name
	ext.extend_from_slice(&ext_payload_len.to_be_bytes());
	ext.extend_from_slice(&list_len.to_be_bytes());
	ext.push(0); // name_type = host_name
	ext.extend_from_slice(&host_name_len.to_be_bytes());
	ext.extend_from_slice(sni_bytes);

	let ext_total = u16::try_from(ext.len()).expect("ext len");
	body.extend_from_slice(&ext_total.to_be_bytes());
	body.extend_from_slice(&ext);

	let body_len = u32::try_from(body.len()).expect("body fits u24");
	let mut msg = vec![
		0x01_u8, // ClientHello
		u8::try_from((body_len >> 16) & 0xff).expect("byte"),
		u8::try_from((body_len >> 8) & 0xff).expect("byte"),
		u8::try_from(body_len & 0xff).expect("byte"),
	];
	msg.extend_from_slice(&body);
	msg
}

fn varint_encode(value: u64) -> Vec<u8> {
	if value < 64 {
		vec![u8::try_from(value).expect("byte")]
	} else if value < 16_384 {
		let v = u16::try_from(value).expect("u16") | 0x4000;
		v.to_be_bytes().to_vec()
	} else if value < 1_073_741_824 {
		let v = u32::try_from(value).expect("u32") | 0x8000_0000;
		v.to_be_bytes().to_vec()
	} else {
		let v = value | 0xc000_0000_0000_0000;
		v.to_be_bytes().to_vec()
	}
}

/// Build a single QUIC v1 Initial datagram carrying a CRYPTO frame
/// (offset 0) with a minimal `ClientHello` whose only extension is
/// `server_name = sni`. AEAD-encrypted under DCID-derived keys with
/// packet number 0.
pub(crate) fn build_initial_datagram_with_sni(dcid: &[u8], sni: &str) -> Vec<u8> {
	let keys = derive_keys(dcid);
	let client_hello = build_client_hello(sni);

	// CRYPTO frame: type 0x06, offset VarInt 0, length VarInt
	// client_hello.len(), then bytes.
	let mut frame: Vec<u8> = Vec::new();
	frame.push(0x06);
	frame.extend_from_slice(&varint_encode(0));
	frame.extend_from_slice(&varint_encode(u64::try_from(client_hello.len()).expect("len")));
	frame.extend_from_slice(&client_hello);

	// Pad payload up to at least 16 bytes after the PN so the header-
	// protection sample (PN_offset + 4 .. + 20) has room to live
	// inside the ciphertext. CRYPTO frames are followed by PADDING
	// bytes (0x00) which decode as no-ops. Make plaintext at least
	// 24 bytes total to guarantee sample availability.
	while frame.len() < 24 {
		frame.push(0x00);
	}

	// Build long-header prefix.
	let mut header: Vec<u8> = Vec::new();
	header.push(0xc0); // long-header, fixed bit, type=Initial, PN length=1
	header.extend_from_slice(&QUIC_V1.to_be_bytes());
	header.push(u8::try_from(dcid.len()).expect("dcid len"));
	header.extend_from_slice(dcid);
	header.push(0); // SCID length
	header.push(0); // token length VarInt = 0

	// Length VarInt covers PN(1) + payload(plaintext.len() + 16-byte tag).
	let length_value = u64::try_from(1 + frame.len() + 16).expect("length");
	header.extend_from_slice(&varint_encode(length_value));

	let pn_offset = header.len();
	header.push(0); // PN = 0 (1 byte)

	// AAD = unprotected header bytes (PN included).
	let aad = header.clone();

	// Encrypt with nonce = iv XOR (zero-padded PN=0) = iv.
	let aead = <Aes128Gcm as KeyInit>::new(GenericArray::from_slice(&keys.key));
	let ciphertext = aead
		.encrypt(GenericArray::from_slice(&keys.iv), aes_gcm::aead::Payload { msg: &frame, aad: &aad })
		.expect("encrypt");

	let mut datagram = header;
	datagram.extend_from_slice(&ciphertext);

	// Apply header protection.
	let sample_offset = pn_offset + 4;
	let sample: [u8; 16] = datagram[sample_offset..sample_offset + 16].try_into().expect("sample");
	let cipher = <Aes128 as KeyInit>::new(GenericArray::from_slice(&keys.hp));
	let mut block = sample;
	cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
	let mask: [u8; 5] = [block[0], block[1], block[2], block[3], block[4]];
	datagram[0] ^= mask[0] & 0x0f;
	datagram[pn_offset] ^= mask[1];

	datagram
}
