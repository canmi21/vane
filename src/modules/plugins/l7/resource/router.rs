/* src/modules/plugins/l7/resource/router.rs */

use anyhow::{Result, anyhow};
use std::path::{Component, Path, PathBuf};

/// Resolves a virtual URI to a physical path, ensuring it stays within the root.
pub fn resolve_path(root: &str, uri_path: &str, allow_symlinks: bool) -> Result<PathBuf> {
	// 1. Percent Decode
	let decoded_uri = percent_encoding::percent_decode_str(uri_path)
		.decode_utf8()
		.map_err(|e| anyhow!("Failed to decode URI: {}", e))?;

	// 2. Normalize & Join (Logical Sanitization)
	// We iterate over components to remove ".." and "." logic purely in memory first.
	// This guarantees 'final_path' has no ".." components syntactically.
	let root_path = Path::new(root)
		.canonicalize()
		.map_err(|e| anyhow!("Root path '{}' is invalid or does not exist: {}", root, e))?;

	let mut final_path = root_path.clone();

	for component in Path::new(decoded_uri.as_ref()).components() {
		match component {
			Component::Normal(c) => final_path.push(c),
			Component::RootDir => {} // Ignore, we explicitly join with root
			Component::CurDir => {}  // Ignore "."
			Component::ParentDir => {
				// Logical Security: Prevent popping above root via ".."
				if final_path > root_path {
					final_path.pop();
				}
			}
			Component::Prefix(_) => {} // Ignore Windows prefixes
		}
	}

	// 3. Jail Check (Symlink Protection)
	if !allow_symlinks {
		// Attempt to resolve the physical path to detect symlink escapes.
		match final_path.canonicalize() {
			Ok(canonical_final) => {
				// File exists and was resolved. Check containment.
				if !canonical_final.starts_with(&root_path) {
					return Err(anyhow!(
						"Forbidden: Path traversal attempt detected via symlink"
					));
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
				return Err(anyhow!("Path Resolution Security Error: {}", e));
			}
		}
	}

	// If symlinks are allowed, we trust the normalized path construction.
	Ok(final_path)
}
