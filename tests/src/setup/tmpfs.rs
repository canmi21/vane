/* tests/src/setup/tmpfs.rs */

use std::io;
use std::path::Path;
use tempfile::TempDir;

/// Manages a temporary directory on the filesystem for test isolation.
///
/// The directory and its contents are automatically deleted when this struct
/// is dropped, providing a clean environment for each test case.
pub struct TmpFs {
	dir: TempDir,
}

impl TmpFs {
	/// Creates a new temporary directory in the system's default location.
	///
	/// # Returns
	///
	/// * `Ok(TmpFs)` on successful creation.
	/// * `Err(io::Error)` if the directory could not be created.
	pub fn new() -> io::Result<Self> {
		let dir = TempDir::new()?;
		Ok(TmpFs { dir })
	}

	/// Returns a reference to the path of the temporary directory.
	pub fn path(&self) -> &Path {
		self.dir.path()
	}

	/// Explicitly consumes the manager and deletes the temporary directory.
	///
	/// While cleanup is automatic on drop, this method allows for capturing
	/// and handling potential I/O errors during deletion.
	pub fn cleanup(self) -> io::Result<()> {
		self.dir.close()
	}
}
