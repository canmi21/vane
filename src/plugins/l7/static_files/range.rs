/* src/plugins/l7/static_files/range.rs */

/// Represents a requested byte range.
#[derive(Debug, Clone, Copy)]
pub struct ByteRange {
	pub start: u64,
	pub length: u64,
}

/// Parses the "Range" header (e.g., "bytes=0-499").
/// Currently supports single ranges and suffix ranges (RFC 9110).
pub fn parse_range_header(header_val: &str, total_size: u64) -> Option<ByteRange> {
	if total_size == 0 || !header_val.starts_with("bytes=") {
		return None;
	}

	let range_part = &header_val[6..];
	let (start_str, end_str) = range_part.split_once('-')?;

	if start_str.is_empty() {
		// Suffix range (e.g., -500)
		let suffix_len = end_str.parse::<u64>().ok()?;
		if suffix_len == 0 {
			return None;
		}
		let start = if suffix_len >= total_size {
			0
		} else {
			total_size - suffix_len
		};
		return Some(ByteRange {
			start,
			length: total_size - start,
		});
	}

	// Normal or Open-ended range
	let start = start_str.parse::<u64>().ok()?;
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_single_range() {
		let res = parse_range_header("bytes=100-199", 1000).unwrap();
		assert_eq!(res.start, 100);
		assert_eq!(res.length, 100);
	}

	#[test]
	fn test_parse_open_ended_range() {
		let res = parse_range_header("bytes=100-", 1000).unwrap();
		assert_eq!(res.start, 100);
		assert_eq!(res.length, 900);
	}

	#[test]
	fn test_parse_suffix_range() {
		// Last 200 bytes
		let res = parse_range_header("bytes=-200", 1000).unwrap();
		assert_eq!(res.start, 800);
		assert_eq!(res.length, 200);

		// Suffix larger than file
		let res2 = parse_range_header("bytes=-5000", 1000).unwrap();
		assert_eq!(res2.start, 0);
		assert_eq!(res2.length, 1000);
	}

	#[test]
	fn test_parse_invalid_ranges() {
		let size = 1000;
		// Out of bounds
		assert!(parse_range_header("bytes=1000-1100", size).is_none());
		// End before start
		assert!(parse_range_header("bytes=500-400", size).is_none());
		// Malformed
		assert!(parse_range_header("items=0-5", size).is_none());
		assert!(parse_range_header("bytes=abc-def", size).is_none());
		assert!(parse_range_header("bytes=100", size).is_none()); // Missing hyphen
	}

	#[test]
	fn test_range_clipping() {
		// End exceeds file size
		let res = parse_range_header("bytes=900-2000", 1000).unwrap();
		assert_eq!(res.start, 900);
		assert_eq!(res.length, 100);
	}
}
