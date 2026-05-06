//! QUIC long-header parsing for Initial packets (RFC 9000 §17.2).
//!
//! Initial packet wire format (long header):
//!
//! ```text
//!   0                   1                   2                   3
//!   0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//!  +-+-+-+-+-+-+-+-+
//!  |1|1|0|0|R R|P P|             first byte (long-header form, type=Initial=00)
//!  +-+-+-+-+-+-+-+-+
//!  |                         Version (32 bits)                     |
//!  +-+-+-+-+-+-+-+-+
//!  | DCID Length   |
//!  +-+-+-+-+-+-+-+-+
//!  |  Destination Connection ID (0..160)                          |
//!  +-+-+-+-+-+-+-+-+
//!  | SCID Length   |
//!  +-+-+-+-+-+-+-+-+
//!  |  Source Connection ID (0..160)                               |
//!  +-+-+-+-+-+-+-+-+
//!  | Token Length VarInt                                           |
//!  +-+-+-+-+-+-+-+-+
//!  | Token (Token Length bytes, may be empty)                      |
//!  +-+-+-+-+-+-+-+-+
//!  | Length VarInt (covers PN + protected payload)                 |
//!  +-+-+-+-+-+-+-+-+
//!  | Packet Number (1..4 bytes; encrypted under header protection) |
//!  +-+-+-+-+-+-+-+-+
//!  | Packet Payload (Length - PN length bytes; encrypted)          |
//!  +-+-+-+-+-+-+-+-+
//! ```

use crate::Error;

/// QUIC v1 transport version (RFC 9000 §15).
pub(crate) const QUIC_V1: u32 = 0x0000_0001;

/// Parsed Initial-packet long header. Byte offsets reference the input
/// datagram and are stable through the AEAD-decrypt pipeline.
#[derive(Debug, Clone)]
pub(crate) struct InitialHeader {
	pub(crate) dcid: Vec<u8>,
	#[allow(dead_code)] // SCID is captured for completeness; not used by SNI extraction.
	pub(crate) scid: Vec<u8>,
	/// Byte offset of the (still header-protected) Packet Number field.
	pub(crate) packet_number_offset: usize,
	/// Value of the Length VarInt: counts Packet Number bytes plus the
	/// protected payload bytes that follow.
	pub(crate) packet_length: usize,
}

impl InitialHeader {
	/// Parse the long-header prefix of a single datagram.
	///
	/// Returns [`Error::NotInitial`] if the first byte's long-header
	/// form bit is unset or the packet type is not Initial,
	/// [`Error::UnsupportedVersion`] for non-v1 versions, and
	/// [`Error::HeaderParse`] for any structural malformation
	/// (truncated CIDs, length overflow, etc.).
	pub(crate) fn parse(datagram: &[u8]) -> Result<Self, Error> {
		// First byte: 1 RFC-defined bit must be set (long form), one
		// fixed bit must be set, and the two type bits must select
		// Initial (00). Reserved bits and PN length bits are still
		// header-protected at this point and we don't inspect them.
		let first = *datagram.first().ok_or(Error::NotInitial)?;
		if first & 0x80 == 0 {
			return Err(Error::NotInitial); // short header
		}
		if first & 0x40 == 0 {
			return Err(Error::NotInitial); // fixed bit must be 1
		}
		// Long-header packet type lives in bits 5-4 (mask 0x30).
		// 00 = Initial, 01 = 0-RTT, 10 = Handshake, 11 = Retry.
		if first & 0x30 != 0x00 {
			return Err(Error::NotInitial);
		}

		let mut idx = 1;
		// Version: 4 bytes big-endian.
		let version = read_u32_be(datagram, idx)?;
		idx += 4;
		if version != QUIC_V1 {
			return Err(Error::UnsupportedVersion(version));
		}

		// DCID length: 1 byte (0..=20 per RFC 9000 §17.2).
		let dcid_len = *datagram.get(idx).ok_or(Error::HeaderParse)? as usize;
		idx += 1;
		if dcid_len > 20 {
			return Err(Error::HeaderParse);
		}
		let dcid_end = idx.checked_add(dcid_len).ok_or(Error::HeaderParse)?;
		let dcid = datagram.get(idx..dcid_end).ok_or(Error::HeaderParse)?.to_vec();
		idx = dcid_end;

		// SCID length: 1 byte (0..=20).
		let scid_len = *datagram.get(idx).ok_or(Error::HeaderParse)? as usize;
		idx += 1;
		if scid_len > 20 {
			return Err(Error::HeaderParse);
		}
		let scid_end = idx.checked_add(scid_len).ok_or(Error::HeaderParse)?;
		let scid = datagram.get(idx..scid_end).ok_or(Error::HeaderParse)?.to_vec();
		idx = scid_end;

		// Token Length: VarInt. Token bytes follow.
		let (token_len, token_len_bytes) = read_varint(datagram, idx)?;
		idx += token_len_bytes;
		let token_len_usize = usize::try_from(token_len).map_err(|_| Error::HeaderParse)?;
		let token_end = idx.checked_add(token_len_usize).ok_or(Error::HeaderParse)?;
		// Confirm token bytes are present in the datagram even though we
		// don't retain them — SNI extraction has no use for the token.
		datagram.get(idx..token_end).ok_or(Error::HeaderParse)?;
		idx = token_end;

		// Length VarInt: covers PN + protected payload.
		let (packet_length_u64, length_bytes) = read_varint(datagram, idx)?;
		idx += length_bytes;
		let packet_length = usize::try_from(packet_length_u64).map_err(|_| Error::HeaderParse)?;
		let payload_end = idx.checked_add(packet_length).ok_or(Error::HeaderParse)?;
		if payload_end > datagram.len() {
			return Err(Error::HeaderParse);
		}

		Ok(Self { dcid, scid, packet_number_offset: idx, packet_length })
	}
}

/// Read a 4-byte big-endian u32, returning [`Error::HeaderParse`] on
/// truncation.
fn read_u32_be(buf: &[u8], offset: usize) -> Result<u32, Error> {
	let bytes: [u8; 4] = buf
		.get(offset..offset + 4)
		.ok_or(Error::HeaderParse)?
		.try_into()
		.map_err(|_| Error::HeaderParse)?;
	Ok(u32::from_be_bytes(bytes))
}

/// QUIC variable-length integer (RFC 9000 §16). Returns the parsed
/// value and the number of bytes consumed.
pub(crate) fn read_varint(buf: &[u8], offset: usize) -> Result<(u64, usize), Error> {
	let first = *buf.get(offset).ok_or(Error::HeaderParse)?;
	let len_log2 = first >> 6;
	let len = 1usize << len_log2;
	let bytes = buf.get(offset..offset + len).ok_or(Error::HeaderParse)?;
	let mut acc: u64 = u64::from(bytes[0] & 0x3f);
	for &b in &bytes[1..] {
		acc = (acc << 8) | u64::from(b);
	}
	Ok((acc, len))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn empty_datagram_returns_not_initial() {
		assert!(matches!(InitialHeader::parse(&[]), Err(Error::NotInitial)));
	}

	#[test]
	fn short_header_returns_not_initial() {
		// First byte high bit unset → short header.
		assert!(matches!(InitialHeader::parse(&[0x40, 0, 0, 0, 1]), Err(Error::NotInitial)));
	}

	#[test]
	fn unsupported_version_v2_returns_unsupported_version() {
		// QUIC v2 version number is 0x6b3343cf (RFC 9369).
		let mut bytes = vec![0xc0]; // long-header, fixed bit set, type=Initial
		bytes.extend_from_slice(&0x6b33_43cf_u32.to_be_bytes());
		bytes.push(0); // dcid len
		bytes.push(0); // scid len
		bytes.push(0); // token len varint = 0
		bytes.push(0); // length varint = 0
		assert!(matches!(InitialHeader::parse(&bytes), Err(Error::UnsupportedVersion(0x6b33_43cf)),));
	}

	#[test]
	fn long_dcid_above_20_bytes_returns_header_parse() {
		let mut bytes = vec![0xc0]; // first byte: long-header Initial
		bytes.extend_from_slice(&QUIC_V1.to_be_bytes());
		bytes.push(21); // illegal DCID length
		assert!(matches!(InitialHeader::parse(&bytes), Err(Error::HeaderParse)));
	}

	#[test]
	fn varint_decodes_one_byte_form() {
		assert_eq!(read_varint(&[0x25], 0).expect("varint"), (37, 1));
	}

	#[test]
	fn varint_decodes_two_byte_form() {
		// 0x4025 → length = 2 (top bits 01), value = 0x0025 = 37
		assert_eq!(read_varint(&[0x40, 0x25], 0).expect("varint"), (37, 2));
	}

	#[test]
	fn varint_decodes_four_byte_form() {
		// 0x80, 0x00, 0x00, 0x25 → length = 4, value = 37
		assert_eq!(read_varint(&[0x80, 0, 0, 0x25], 0).expect("varint"), (37, 4));
	}

	#[test]
	fn varint_decodes_eight_byte_form() {
		// 0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c → length=8
		// Value = 0x0219_7c5e_ff14_e88c per RFC 9000 §16 example.
		let bytes = [0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c];
		assert_eq!(read_varint(&bytes, 0).expect("varint"), (0x0219_7c5e_ff14_e88c, 8));
	}
}
