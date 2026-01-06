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

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::NamedTempFile;

	#[tokio::test]
	async fn test_mime_by_extension() {
		// 1. HTML
		let f = NamedTempFile::new_in(".").unwrap();
		let path = Path::new("index.html"); // We only need the path for extension guess
		let mut file = File::open(f.path()).await.unwrap();
		assert_eq!(determine_mime_type(path, &mut file).await, "text/html");

		// 2. JSON
		let path = Path::new("data.json");
		assert_eq!(
			determine_mime_type(path, &mut file).await,
			"application/json"
		);

		// 3. Image (Case Insensitive)
		let path = Path::new("PHOTO.JPG");
		assert_eq!(determine_mime_type(path, &mut file).await, "image/jpeg");
	}

	#[tokio::test]
	async fn test_mime_by_sniffing() {
		// Create a file with NO extension but PNG content
		let tmp = NamedTempFile::new().unwrap();
		// PNG Magic Bytes: 89 50 4E 47 0D 0A 1A 0A
		let png_data = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
		std::fs::write(tmp.path(), &png_data).unwrap();

		let mut file = File::open(tmp.path()).await.unwrap();
		let path = Path::new("unknown_file");

		assert_eq!(determine_mime_type(path, &mut file).await, "image/png");
	}

	#[tokio::test]
	async fn test_mime_fallback_text() {
		let tmp = NamedTempFile::new().unwrap();
		std::fs::write(tmp.path(), b"Just some plain text content").unwrap();

		let mut file = File::open(tmp.path()).await.unwrap();
		let path = Path::new("README");

		assert_eq!(determine_mime_type(path, &mut file).await, "text/plain");
	}

	#[tokio::test]
	async fn test_mime_fallback_octet_stream() {
		let tmp = NamedTempFile::new().unwrap();
		// Random binary data that doesn't match any magic bytes
		let binary_data = [0x00, 0x01, 0x02, 0x03, 0xFF, 0xFE];
		std::fs::write(tmp.path(), &binary_data).unwrap();

		let mut file = File::open(tmp.path()).await.unwrap();
		let path = Path::new("binary.data");

		assert_eq!(
			determine_mime_type(path, &mut file).await,
			"application/octet-stream"
		);
	}

	#[test]
	fn test_etag_generation() {
		let t1 = std::time::UNIX_EPOCH + std::time::Duration::from_secs(100);
		let etag = generate_etag(t1, 500);
		// 100 secs = 100,000,000,000 nanos = 174876e800 hex
		// 500 = 1f4 hex
		assert!(etag.starts_with("W/\""));
		assert!(etag.contains("1f4"));
	}
}
