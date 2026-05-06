//! QUIC frame walker for Initial-packet plaintext payloads.
//!
//! Per RFC 9000 §17.2.2, an Initial packet's plaintext may carry only:
//!   - PADDING (0x00)
//!   - PING    (0x01)
//!   - ACK     (0x02 / 0x03)
//!   - CRYPTO  (0x06)
//!   - CONNECTION_CLOSE (0x1c — type 0x1d is for application errors,
//!     which is forbidden in Initial packets)
//!
//! Anything else is a protocol violation and surfaces as
//! [`Error::FrameDecode`].

use crate::Error;
use crate::header::read_varint;

/// One decoded CRYPTO frame's stream contribution.
#[derive(Debug, Clone)]
pub(crate) struct CryptoSegment {
	pub(crate) offset: u64,
	pub(crate) data: Vec<u8>,
}

/// Walk every frame in `payload` and return the CRYPTO contributions
/// in encounter order. Other allowed frame types (PADDING / PING /
/// ACK / CONNECTION_CLOSE) are recognised and skipped; disallowed
/// frame types fail with [`Error::FrameDecode`].
pub(crate) fn collect_crypto_segments(payload: &[u8]) -> Result<Vec<CryptoSegment>, Error> {
	let mut idx = 0;
	let mut out = Vec::new();
	while idx < payload.len() {
		let frame_type = *payload.get(idx).ok_or(Error::FrameDecode)?;
		idx += 1;
		match frame_type {
			// PADDING (0x00) — single byte, may run for any number of
			// bytes. PING (0x01) — single byte. Both have empty bodies.
			0x00 | 0x01 => {}
			0x02 | 0x03 => {
				// ACK frame: largest, delay, range count, first range,
				// then range count × (gap, length). 0x03 adds ECN
				// counts (ect0, ect1, ce). All VarInts.
				idx = skip_ack_frame(payload, idx, frame_type == 0x03)?;
			}
			0x06 => {
				// CRYPTO: offset (VarInt), length (VarInt), data.
				let (offset, off_len) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
				idx += off_len;
				let (length_u64, len_bytes) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
				idx += len_bytes;
				let length = usize::try_from(length_u64).map_err(|_| Error::FrameDecode)?;
				let end = idx.checked_add(length).ok_or(Error::FrameDecode)?;
				let data = payload.get(idx..end).ok_or(Error::FrameDecode)?.to_vec();
				idx = end;
				out.push(CryptoSegment { offset, data });
			}
			0x1c => {
				// CONNECTION_CLOSE (transport error): error code (VarInt),
				// frame type (VarInt), reason length (VarInt), reason.
				let (_err, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
				idx += n;
				let (_ftype, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
				idx += n;
				let (reason_len_u64, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
				idx += n;
				let reason_len = usize::try_from(reason_len_u64).map_err(|_| Error::FrameDecode)?;
				let end = idx.checked_add(reason_len).ok_or(Error::FrameDecode)?;
				payload.get(idx..end).ok_or(Error::FrameDecode)?;
				idx = end;
			}
			_ => return Err(Error::FrameDecode),
		}
	}
	Ok(out)
}

fn skip_ack_frame(payload: &[u8], start: usize, has_ecn: bool) -> Result<usize, Error> {
	let mut idx = start;
	let (_largest, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
	idx += n;
	let (_delay, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
	idx += n;
	let (range_count, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
	idx += n;
	let (_first, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
	idx += n;
	for _ in 0..range_count {
		let (_gap, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
		idx += n;
		let (_length, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
		idx += n;
	}
	if has_ecn {
		for _ in 0..3 {
			let (_count, n) = read_varint(payload, idx).map_err(|_| Error::FrameDecode)?;
			idx += n;
		}
	}
	Ok(idx)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn padding_only_payload_yields_no_segments() {
		let payload = vec![0u8; 32];
		assert!(collect_crypto_segments(&payload).expect("walk").is_empty());
	}

	#[test]
	fn single_crypto_frame_decoded() {
		// CRYPTO type=0x06, offset VarInt=0 (1 byte), length VarInt=4 (1 byte),
		// data = "test".
		let payload = vec![0x06, 0x00, 0x04, b't', b'e', b's', b't'];
		let segs = collect_crypto_segments(&payload).expect("walk");
		assert_eq!(segs.len(), 1);
		assert_eq!(segs[0].offset, 0);
		assert_eq!(segs[0].data, b"test");
	}

	#[test]
	fn unknown_frame_type_in_initial_returns_frame_decode() {
		// Type 0x08 = STREAM frame, which is forbidden in Initial.
		let payload = vec![0x08];
		assert!(matches!(collect_crypto_segments(&payload), Err(Error::FrameDecode)));
	}

	#[test]
	fn ping_then_crypto_walks_both() {
		let payload = vec![0x01, 0x06, 0x00, 0x02, 0xab, 0xcd];
		let segs = collect_crypto_segments(&payload).expect("walk");
		assert_eq!(segs.len(), 1);
		assert_eq!(segs[0].data, vec![0xab, 0xcd]);
	}

	#[test]
	fn ack_frame_skipped() {
		// ACK 0x02: largest=5, delay=0, range_count=0, first_range=5.
		let payload = vec![0x02, 0x05, 0x00, 0x00, 0x05];
		assert!(collect_crypto_segments(&payload).expect("walk").is_empty());
	}
}
