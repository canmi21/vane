//! TLS 1.3 ClientHello parser, scoped to the SNI extension.
//!
//! QUIC carries the ClientHello directly inside CRYPTO frames — no
//! TLS record-layer header. So the parser starts at:
//!
//!   HandshakeMessage:
//!     msg_type:   u8   == 0x01 (ClientHello)
//!     length:     u24  == body length
//!     body:
//!       legacy_version          u16
//!       random                  [u8; 32]
//!       session_id_length       u8     (0..=32)
//!       session_id              [u8; session_id_length]
//!       cipher_suites_length    u16    (must be even, > 0)
//!       cipher_suites           [u16; n]
//!       compression_length      u8
//!       compression_methods     [u8; m]
//!       extensions_length       u16
//!       extensions              [u8; k]
//!
//! The server_name extension (type 0x0000, RFC 6066 §3) wraps a list
//! of `ServerName` entries; only `name_type = 0` (host_name) is
//! defined. The host_name is a length-prefixed UTF-8 string.

use crate::Error;

const HANDSHAKE_TYPE_CLIENT_HELLO: u8 = 0x01;
const EXT_SERVER_NAME: u16 = 0x0000;
const NAME_TYPE_HOST_NAME: u8 = 0x00;

/// Try to extract the SNI host name from a buffered ClientHello prefix.
///
/// Returns:
///   * `Ok(Some(sni))` when a complete ClientHello is parsed and the
///     SNI extension carries a host_name.
///   * `Ok(None)` when the buffer is shorter than the declared
///     ClientHello body — more bytes needed.
///   * `Err(_)` when the structure is malformed or the SNI extension
///     is present but contains no host_name.
pub(crate) fn try_extract_sni(buf: &[u8]) -> Result<Option<String>, Error> {
	if buf.len() < 4 {
		return Ok(None);
	}
	if buf[0] != HANDSHAKE_TYPE_CLIENT_HELLO {
		return Err(Error::TlsParse);
	}
	let body_len = (usize::from(buf[1]) << 16) | (usize::from(buf[2]) << 8) | usize::from(buf[3]);
	let total_needed = 4 + body_len;
	if buf.len() < total_needed {
		return Ok(None);
	}
	let body = &buf[4..total_needed];
	parse_client_hello_body(body).map(Some)
}

fn parse_client_hello_body(body: &[u8]) -> Result<String, Error> {
	let mut idx: usize = 0;
	// legacy_version (2) + random (32)
	idx = idx.checked_add(34).ok_or(Error::TlsParse)?;
	if body.len() < idx {
		return Err(Error::TlsParse);
	}
	// session_id
	let sid_len = usize::from(*body.get(idx).ok_or(Error::TlsParse)?);
	idx += 1;
	if sid_len > 32 {
		return Err(Error::TlsParse);
	}
	idx = idx.checked_add(sid_len).ok_or(Error::TlsParse)?;
	// cipher_suites
	let cs_len = read_u16(body, idx)?;
	idx += 2;
	if cs_len % 2 != 0 {
		return Err(Error::TlsParse);
	}
	idx = idx.checked_add(usize::from(cs_len)).ok_or(Error::TlsParse)?;
	// compression_methods
	let comp_len = usize::from(*body.get(idx).ok_or(Error::TlsParse)?);
	idx += 1;
	idx = idx.checked_add(comp_len).ok_or(Error::TlsParse)?;
	// extensions
	let ext_total = usize::from(read_u16(body, idx)?);
	idx += 2;
	let ext_end = idx.checked_add(ext_total).ok_or(Error::TlsParse)?;
	if ext_end > body.len() {
		return Err(Error::TlsParse);
	}
	let extensions = &body[idx..ext_end];

	parse_extensions_for_sni(extensions)
}

fn parse_extensions_for_sni(extensions: &[u8]) -> Result<String, Error> {
	let mut idx = 0;
	while idx < extensions.len() {
		let ext_type = read_u16(extensions, idx)?;
		idx += 2;
		let ext_len = usize::from(read_u16(extensions, idx)?);
		idx += 2;
		let ext_end = idx.checked_add(ext_len).ok_or(Error::TlsParse)?;
		if ext_end > extensions.len() {
			return Err(Error::TlsParse);
		}
		if ext_type == EXT_SERVER_NAME {
			return parse_server_name_extension(&extensions[idx..ext_end]);
		}
		idx = ext_end;
	}
	Err(Error::TlsParse)
}

fn parse_server_name_extension(payload: &[u8]) -> Result<String, Error> {
	// ServerNameList: list_length (u16), then entries.
	let list_len = usize::from(read_u16(payload, 0)?);
	let list_end = 2usize.checked_add(list_len).ok_or(Error::TlsParse)?;
	if list_end > payload.len() {
		return Err(Error::TlsParse);
	}
	let list = &payload[2..list_end];
	let mut idx = 0;
	while idx < list.len() {
		let name_type = *list.get(idx).ok_or(Error::TlsParse)?;
		idx += 1;
		let name_len = usize::from(read_u16(list, idx)?);
		idx += 2;
		let name_end = idx.checked_add(name_len).ok_or(Error::TlsParse)?;
		if name_end > list.len() {
			return Err(Error::TlsParse);
		}
		let name_bytes = &list[idx..name_end];
		if name_type == NAME_TYPE_HOST_NAME {
			return std::str::from_utf8(name_bytes).map(str::to_owned).map_err(|_| Error::TlsParse);
		}
		idx = name_end;
	}
	Err(Error::TlsParse)
}

fn read_u16(buf: &[u8], offset: usize) -> Result<u16, Error> {
	let bytes: [u8; 2] =
		buf.get(offset..offset + 2).ok_or(Error::TlsParse)?.try_into().map_err(|_| Error::TlsParse)?;
	Ok(u16::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Build a minimal ClientHello with the given SNI host_name.
	/// Cipher suite list is `[0x1301]` (TLS_AES_128_GCM_SHA256), one
	/// compression method (null). `host_name` is the only extension.
	fn build_client_hello(sni: &str) -> Vec<u8> {
		let mut body: Vec<u8> = Vec::new();
		// legacy_version
		body.extend_from_slice(&[0x03, 0x03]);
		// random
		body.extend_from_slice(&[0u8; 32]);
		// session_id
		body.push(0);
		// cipher_suites
		body.extend_from_slice(&2u16.to_be_bytes());
		body.extend_from_slice(&[0x13, 0x01]);
		// compression
		body.push(1);
		body.push(0);
		// extensions
		let mut ext_body: Vec<u8> = Vec::new();
		// server_name extension
		let sni_bytes = sni.as_bytes();
		let host_name_len = u16::try_from(sni_bytes.len()).expect("sni fits u16");
		let server_name_entry_len = 1 + 2 + host_name_len; // name_type + len + bytes
		let list_len = server_name_entry_len; // single entry
		let ext_payload_len = 2 + list_len; // list_length prefix + list
		ext_body.extend_from_slice(&EXT_SERVER_NAME.to_be_bytes());
		ext_body.extend_from_slice(&ext_payload_len.to_be_bytes());
		ext_body.extend_from_slice(&list_len.to_be_bytes());
		ext_body.push(NAME_TYPE_HOST_NAME);
		ext_body.extend_from_slice(&host_name_len.to_be_bytes());
		ext_body.extend_from_slice(sni_bytes);
		body.extend_from_slice(&u16::try_from(ext_body.len()).expect("ext fits u16").to_be_bytes());
		body.extend_from_slice(&ext_body);

		// Wrap in HandshakeMessage envelope. `body_len` is bounded by
		// our test fixtures (well under 2 KB) so `try_from` for the
		// 24-bit big-endian length encoding never trips.
		let body_len = body.len();
		let body_len_u32 = u32::try_from(body_len).expect("test fixture body fits u24");
		let mut msg = Vec::with_capacity(4 + body_len);
		msg.push(HANDSHAKE_TYPE_CLIENT_HELLO);
		msg.push(u8::try_from((body_len_u32 >> 16) & 0xff).expect("byte"));
		msg.push(u8::try_from((body_len_u32 >> 8) & 0xff).expect("byte"));
		msg.push(u8::try_from(body_len_u32 & 0xff).expect("byte"));
		msg.extend_from_slice(&body);
		msg
	}

	#[test]
	fn extracts_sni_from_minimal_client_hello() {
		let hello = build_client_hello("example.com");
		let sni = try_extract_sni(&hello).expect("parse").expect("present");
		assert_eq!(sni, "example.com");
	}

	#[test]
	fn truncated_client_hello_returns_need_more() {
		let hello = build_client_hello("api.example.org");
		let truncated = &hello[..hello.len() - 5];
		assert!(matches!(try_extract_sni(truncated), Ok(None)));
	}

	#[test]
	fn non_client_hello_handshake_type_returns_tls_parse() {
		let mut bytes = build_client_hello("nope.example");
		bytes[0] = 0x02; // ServerHello — not allowed at this slot
		assert!(matches!(try_extract_sni(&bytes), Err(Error::TlsParse)));
	}

	#[test]
	fn extracts_long_sni() {
		let long = "very.long.subdomain.with.many.parts.example.test";
		let hello = build_client_hello(long);
		let sni = try_extract_sni(&hello).expect("parse").expect("present");
		assert_eq!(sni, long);
	}
}
