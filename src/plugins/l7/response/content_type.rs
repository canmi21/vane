/* src/plugins/l7/response/content_type.rs */

use bytes::Bytes;
use serde_json::Value;

pub fn guess_mime(bytes: &Bytes) -> &'static str {
	// Try to detect via magic bytes (covers images, archives, etc.)
	if let Some(kind) = infer::get(bytes) {
		return kind.mime_type();
	}

	// If unknown, check for valid UTF-8 text
	if let Ok(text) = std::str::from_utf8(bytes) {
		let trimmed = text.trim_start();

		// HTML narrow window detection
		if trimmed.starts_with("<!DOCTYPE html")
			|| trimmed.starts_with("<!doctype html")
			|| trimmed.starts_with("<html")
			|| trimmed.starts_with("<HTML")
			|| (trimmed.starts_with("<?xml") && trimmed.contains("<html"))
		{
			return "text/html; charset=utf-8";
		}

		// JSON detection
		let start_byte = bytes.iter().find(|&&b| !b.is_ascii_whitespace());
		if let Some(&b'{' | &b'[') = start_byte
			&& serde_json::from_slice::<Value>(bytes).is_ok() {
				return "application/json";
			}

		// Fallback for valid text that isn't JSON or HTML
		return "text/plain; charset=utf-8";
	}

	// Default for unknown binary data
	"application/octet-stream"
}
