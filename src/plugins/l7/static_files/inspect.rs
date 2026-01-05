/* src/plugins/l7/static_files/inspect.rs */

use crate::common::config::env_loader;
use std::path::Path;
use tokio::{
	fs::File,
	io::{AsyncReadExt, AsyncSeekExt, SeekFrom},
};

/// Guesses the MIME type of a file.
/// Strategy:
/// 1. Extension-based guess (mime_guess).
/// 2. If unknown/fallback, read first N bytes and use magic bytes (infer).
/// 3. Default to "application/octet-stream".
pub async fn determine_mime_type(path: &Path, file: &mut File) -> String {
	// 1. Extension Guess
	let mime_guess = mime_guess::from_path(path).first();

	if let Some(mime) = mime_guess {
		if mime.type_() != "application" || mime.subtype() != "octet-stream" {
			return mime.to_string();
		}
	}

	// 2. Magic Bytes Sniffing (Fallback)
	let sniff_len_str = env_loader::get_env("STATIC_MIME_SNIFF_BYTES", "512".to_string());
	let sniff_len: usize = sniff_len_str.parse().unwrap_or(512);

	let mut buf = vec![0u8; sniff_len];

	// Save current position (should be 0, but safety first)
	let current_pos = file.stream_position().await.unwrap_or(0);

	let read_len = match file.read(&mut buf).await {
		Ok(n) => n,
		Err(_) => 0,
	};

	// Restore position so subsequent reads aren't affected
	let _ = file.seek(SeekFrom::Start(current_pos)).await;

	if read_len > 0 {
		// Use `infer` to detect type from bytes
		if let Some(kind) = infer::get(&buf[..read_len]) {
			return kind.mime_type().to_string();
		}

		// Simple Text vs Binary heuristic if infer failed
		if std::str::from_utf8(&buf[..read_len]).is_ok() {
			return "text/plain".to_string();
		}
	}

	"application/octet-stream".to_string()
}

/// Generates a weak ETag based on file metadata.
/// Format: W/"<mtime_nanos>-<size>"
pub fn generate_etag(modified: std::time::SystemTime, size: u64) -> String {
	let duration = modified
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default();
	format!("W/\"{:x}-{:x}\"", duration.as_nanos(), size)
}
