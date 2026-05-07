//! CRYPTO-stream byte reassembly keyed by offset.
//!
//! QUIC CRYPTO frames carry an offset + length + bytes triple. The
//! `ClientHello` may span multiple frames possibly across multiple
//! Initial packets, with arrival order independent of offset order.
//! This reassembler maintains a sorted, non-overlapping view and
//! produces the contiguous prefix from offset 0 on demand.
//!
//! Overlapping ranges with **identical** bytes are silently merged
//! (the spec allows benign retransmission). Overlaps with **conflicting**
//! bytes raise [`Error::ConflictingOverlap`] — a well-behaved client
//! never retransmits Initial CRYPTO ranges with different content, so
//! a conflict is treated as adversarial.

use std::collections::BTreeMap;

use subtle::ConstantTimeEq;

use crate::Error;

#[derive(Debug, Default)]
pub(crate) struct CryptoStream {
	/// Sorted by offset; values are owned bytes belonging to that
	/// offset. Adjacent / overlapping segments are merged on insert
	/// so the contiguous-prefix scan is O(prefix segments).
	segments: BTreeMap<u64, Vec<u8>>,
}

impl CryptoStream {
	pub(crate) fn new() -> Self {
		Self::default()
	}

	/// Total bytes currently buffered across all segments.
	pub(crate) fn total_bytes(&self) -> usize {
		self.segments.values().map(Vec::len).sum()
	}

	/// Insert one segment into the stream.
	///
	/// Returns [`Error::ConflictingOverlap`] if this segment overlaps
	/// any existing segment with non-identical bytes.
	pub(crate) fn push(&mut self, offset: u64, data: &[u8]) -> Result<(), Error> {
		if data.is_empty() {
			return Ok(());
		}

		let new_end = offset
			.checked_add(u64::try_from(data.len()).map_err(|_| Error::FrameDecode)?)
			.ok_or(Error::FrameDecode)?;

		// Confirm any pre-existing segment overlapping [offset, new_end)
		// has matching bytes. `try_from` keeps the u64→usize narrowing
		// contractual; the caller's per-session 16 KiB bound puts every
		// real value well below `usize::MAX` even on 32-bit targets.
		for (&seg_off, seg_data) in &self.segments {
			let seg_end = seg_off + seg_data.len() as u64;
			if seg_end <= offset || seg_off >= new_end {
				continue;
			}
			let overlap_start = offset.max(seg_off);
			let overlap_end = new_end.min(seg_end);
			let new_slice_start =
				usize::try_from(overlap_start - offset).map_err(|_| Error::FrameDecode)?;
			let new_slice_end = usize::try_from(overlap_end - offset).map_err(|_| Error::FrameDecode)?;
			let seg_slice_start =
				usize::try_from(overlap_start - seg_off).map_err(|_| Error::FrameDecode)?;
			let seg_slice_end = usize::try_from(overlap_end - seg_off).map_err(|_| Error::FrameDecode)?;
			let new_slice = &data[new_slice_start..new_slice_end];
			let seg_slice = &seg_data[seg_slice_start..seg_slice_end];
			// Constant-time compare to keep adversarial timing channels
			// closed off — overlap conflicts already imply suspect
			// peer behavior.
			if !bool::from(new_slice.ct_eq(seg_slice)) {
				return Err(Error::ConflictingOverlap);
			}
		}

		// Insert and merge with overlapping / adjacent neighbors.
		// Build the merged span by walking neighbors in range and
		// folding their bytes into a single contiguous Vec.
		let mut merged_start = offset;
		let mut merged_end = new_end;
		let mut absorb: Vec<u64> = Vec::new();
		for (&seg_off, seg_data) in &self.segments {
			let seg_end = seg_off + seg_data.len() as u64;
			if seg_end < merged_start || seg_off > merged_end {
				continue;
			}
			absorb.push(seg_off);
			merged_start = merged_start.min(seg_off);
			merged_end = merged_end.max(seg_end);
		}

		let merged_len = usize::try_from(merged_end - merged_start).map_err(|_| Error::FrameDecode)?;
		let mut merged = vec![0u8; merged_len];

		// Place existing absorbed segments first, then overlay the new
		// data. Overlapping bytes were validated equal above, so order
		// of write doesn't change the result.
		for off in &absorb {
			let seg_data = self.segments.remove(off).expect("present");
			let local_off = usize::try_from(*off - merged_start).map_err(|_| Error::FrameDecode)?;
			merged[local_off..local_off + seg_data.len()].copy_from_slice(&seg_data);
		}
		let local_off = usize::try_from(offset - merged_start).map_err(|_| Error::FrameDecode)?;
		merged[local_off..local_off + data.len()].copy_from_slice(data);

		self.segments.insert(merged_start, merged);
		Ok(())
	}

	/// Return the contiguous prefix from offset 0, or `None` if the
	/// stream is empty / starts after offset 0.
	pub(crate) fn contiguous_prefix(&self) -> Option<&[u8]> {
		self.segments.get(&0).map(Vec::as_slice)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn empty_stream_has_no_prefix() {
		let s = CryptoStream::new();
		assert!(s.contiguous_prefix().is_none());
	}

	#[test]
	fn single_segment_at_offset_zero_yields_prefix() {
		let mut s = CryptoStream::new();
		s.push(0, &[1, 2, 3]).expect("push");
		assert_eq!(s.contiguous_prefix(), Some(&[1u8, 2, 3][..]));
	}

	#[test]
	fn segment_starting_after_zero_is_buffered_but_not_in_prefix() {
		let mut s = CryptoStream::new();
		s.push(5, &[9, 9]).expect("push");
		assert!(s.contiguous_prefix().is_none());
		assert_eq!(s.total_bytes(), 2);
	}

	#[test]
	fn out_of_order_segments_reassemble_contiguously() {
		let mut s = CryptoStream::new();
		s.push(3, &[4, 5, 6]).expect("push 3");
		s.push(0, &[1, 2, 3]).expect("push 0");
		assert_eq!(s.contiguous_prefix(), Some(&[1u8, 2, 3, 4, 5, 6][..]));
	}

	#[test]
	fn identical_overlap_silently_merges() {
		let mut s = CryptoStream::new();
		s.push(0, &[1, 2, 3, 4]).expect("push 1");
		s.push(2, &[3, 4, 5, 6]).expect("push 2 — overlap matches");
		assert_eq!(s.contiguous_prefix(), Some(&[1u8, 2, 3, 4, 5, 6][..]));
	}

	#[test]
	fn conflicting_overlap_returns_error() {
		let mut s = CryptoStream::new();
		s.push(0, &[1, 2, 3, 4]).expect("push 1");
		assert!(matches!(s.push(2, &[9, 9, 5, 6]), Err(Error::ConflictingOverlap)));
	}

	#[test]
	fn empty_segment_is_a_no_op() {
		let mut s = CryptoStream::new();
		s.push(10, &[]).expect("empty push");
		assert_eq!(s.total_bytes(), 0);
	}
}
