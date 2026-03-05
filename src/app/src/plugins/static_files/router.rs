/* src/app/src/plugins/static_files/router.rs */

use anyhow::{Result, anyhow};
use std::path::{Component, Path, PathBuf};

/// Resolves a virtual URI to a physical path, ensuring it stays within the root.
pub fn resolve_path(root: &str, uri_path: &str, allow_symlinks: bool) -> Result<PathBuf> {
	// 1. Percent Decode
	let decoded_uri = percent_encoding::percent_decode_str(uri_path)
		.decode_utf8()
		.map_err(|e| anyhow!("Failed to decode URI: {e}"))?;

	// 2. Normalize & Join (Logical Sanitization)
	// We iterate over components to remove ".." and "." logic purely in memory first.
	// This guarantees 'final_path' has no ".." components syntactically.
	let root_path = Path::new(root)
		.canonicalize()
		.map_err(|e| anyhow!("Root path '{root}' is invalid or does not exist: {e}"))?;

	let mut final_path = root_path.clone();

	for component in Path::new(decoded_uri.as_ref()).components() {
		match component {
			Component::Normal(c) => final_path.push(c),
			Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
			Component::ParentDir => {
				// Logical Security: Prevent popping above root via ".."
				if final_path > root_path {
					final_path.pop();
				}
			}
		}
	}

	// 3. Jail Check (Symlink Protection)
	if !allow_symlinks {
		// Attempt to resolve the physical path to detect symlink escapes.
		match final_path.canonicalize() {
			Ok(canonical_final) => {
				// File exists and was resolved. Check containment.
				if !canonical_final.starts_with(&root_path) {
					return Err(anyhow!("Forbidden: Path traversal attempt detected via symlink"));
				}
				return Ok(canonical_final);
			}
			Err(e) => {
				// STRICT ERROR HANDLING:
				// If the error is NotFound, it means the file physically doesn't exist.
				// Since Step 2 guaranteed the path is syntactically clean (no ".."),
				// it is safe to return the logical path. The plugin will later fail with 404.
				if e.kind() == std::io::ErrorKind::NotFound {
					return Ok(final_path);
				}

				// For ALL other errors (PermissionDenied, Loop, etc.), we MUST fail.
				// Fallback is strictly forbidden here to prevents security bypasses.
				return Err(anyhow!("Path Resolution Security Error: {e}"));
			}
		}
	}

	// If symlinks are allowed, we trust the normalized path construction.
	Ok(final_path)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs;
	use tempfile::tempdir;

	#[test]
	fn test_resolve_safe_path() {
		let dir = tempdir().unwrap();
		let root_canonical = dir.path().canonicalize().unwrap();
		let root = root_canonical.to_str().unwrap();
		let file_name = "test.txt";
		fs::write(dir.path().join(file_name), "hello").unwrap();

		let resolved = resolve_path(root, "/test.txt", false).unwrap();
		assert!(resolved.ends_with(file_name));
		assert!(resolved.exists());
	}

	#[test]
	fn test_path_traversal_prevention() {
		let dir = tempdir().unwrap();
		let root_canonical = dir.path().canonicalize().unwrap();
		let root = root_canonical.to_str().unwrap();

		// Case 1: Simple syntactic traversal
		let resolved = resolve_path(root, "/../../etc/passwd", false).unwrap();
		// Should be root/etc/passwd (normalized but confined to root)
		assert!(resolved.starts_with(&root_canonical));
		assert!(!resolved.to_str().unwrap().contains(".."));

		// Case 2: Percent encoded traversal
		let resolved_enc = resolve_path(root, "/%2e%2e/%2e%2e/etc/shadow", false).unwrap();
		assert!(resolved_enc.starts_with(&root_canonical));
		assert!(resolved_enc.to_str().unwrap().contains("etc/shadow"));
	}

	#[test]
	fn test_path_normalization() {
		let dir = tempdir().unwrap();
		let root_canonical = dir.path().canonicalize().unwrap();
		let root = root_canonical.to_str().unwrap();

		// Redundant slashes and current dir
		let resolved = resolve_path(root, "/assets//images/./logo.png", false).unwrap();
		let expected_suffix = Path::new("assets").join("images").join("logo.png");
		assert!(resolved.ends_with(expected_suffix));
	}

	#[test]
	fn test_non_existent_path_safety() {
		let dir = tempdir().unwrap();
		let root_canonical = dir.path().canonicalize().unwrap();
		let root = root_canonical.to_str().unwrap();

		// File doesn't exist but path is clean
		let resolved = resolve_path(root, "/missing.html", false).unwrap();
		assert!(resolved.starts_with(&root_canonical));
		assert!(!resolved.exists());
	}

	#[test]
	fn test_absolute_uri_denial() {
		let dir = tempdir().unwrap();
		let root_canonical = dir.path().canonicalize().unwrap();
		let root = root_canonical.to_str().unwrap();

		// On Unix, /etc/passwd is an absolute path component
		let resolved = resolve_path(root, "/etc/passwd", false).unwrap();
		// It should be treated relative to root: root/etc/passwd
		assert!(resolved.starts_with(&root_canonical));
		assert!(resolved.to_str().unwrap().contains("etc/passwd"));
	}

	#[cfg(unix)]
	#[test]
	fn test_symlink_traversal_prevention() {
		let dir = tempdir().unwrap();
		let root_dir = dir.path().join("www");
		fs::create_dir(&root_dir).unwrap();
		let root = root_dir.to_str().unwrap();

		// Create a file outside root
		let secret_file = dir.path().join("secret.txt");
		fs::write(&secret_file, "top secret").unwrap();

		// Create a symlink inside root pointing to outside
		let link_path = root_dir.join("malicious_link");
		std::os::unix::fs::symlink(&secret_file, &link_path).unwrap();

		// Attempt to resolve the link with allow_symlinks=false
		let res = resolve_path(root, "/malicious_link", false);
		assert!(res.is_err(), "Symlink traversal should be blocked");
		assert!(res.unwrap_err().to_string().contains("Path traversal attempt detected"));

		// Should succeed if explicitly allowed
		let res_allowed = resolve_path(root, "/malicious_link", true);
		assert!(res_allowed.is_ok());
	}
}
