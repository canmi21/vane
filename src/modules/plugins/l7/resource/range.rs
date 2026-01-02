/* src/modules/plugins/l7/resource/range.rs */

/// Represents a requested byte range.
#[derive(Debug, Clone, Copy)]
pub struct ByteRange {
	pub start: u64,
	pub length: u64,
}

/// Parses the "Range" header (e.g., "bytes=0-499").
/// Currently supports only simple single ranges (the most common case).
pub fn parse_range_header(header_val: &str, total_size: u64) -> Option<ByteRange> {
	if !header_val.starts_with("bytes=") {
		return None;
	}

	let range_part = &header_val[6..];
	let (start_str, end_str) = range_part.split_once('-')?;

	let start = if start_str.is_empty() {
		// suffix-byte-range-spec (e.g. -500 means last 500 bytes)
		// Not implemented for simplicity in V0.6, focusing on byte-range-spec
		return None;
	} else {
		start_str.parse::<u64>().ok()?
	};

	let end = if end_str.is_empty() {
		total_size - 1
	} else {
		end_str.parse::<u64>().ok()?
	};

	if start > end || start >= total_size {
		return None; // Unsatisfiable
	}

	// Cap end at EOF
	let final_end = std::cmp::min(end, total_size - 1);

	Some(ByteRange {
		start,
		length: final_end - start + 1,
	})
}
