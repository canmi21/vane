/* tests/src/setup/tmpenv.rs */

use super::tmpfs::TmpFs;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Manages a temporary environment consisting of a directory with a .env file.
///
/// This struct leverages TmpFs to create an isolated directory and then writes
/// a .env file into it. The entire directory is cleaned up when the instance
/// is dropped.
pub struct TmpEnv {
	fs: TmpFs,
	env_path: PathBuf,
}

impl TmpEnv {
	/// Creates a new temporary environment with a .env file.
	///
	/// * `key_values` - A slice of key-value tuples to be written into the
	///   .env file. Keys are automatically converted to uppercase.
	///
	/// # Returns
	///
	/// * `Ok(TmpEnv)` on successful creation of the directory and file.
	/// * `Err(io::Error)` if any filesystem operation fails.
	pub fn new<K, V>(key_values: &[(K, V)]) -> io::Result<Self>
	where
		K: AsRef<str>,
		V: AsRef<str>,
	{
		let fs = TmpFs::new()?;
		let env_path = fs.path().join(".env");

		let content = key_values
			.iter()
			.map(|(key, value)| format!("{}={}", key.as_ref().to_uppercase(), value.as_ref()))
			.collect::<Vec<_>>()
			.join("\n");

		// Ensure file ends with a newline if not empty
		let final_content = if content.is_empty() {
			content
		} else {
			format!("{}\n", content)
		};

		fs::write(&env_path, final_content)?;

		Ok(TmpEnv { fs, env_path })
	}

	/// Sets or updates a key-value pair in the .env file.
	///
	/// This function reads the file, replaces the line containing the key if it
	/// exists, or appends a new line if it does not. The key is case-insensitive
	/// for matching but will be written in uppercase.
	pub fn set<K, V>(&mut self, key: K, value: V) -> io::Result<()>
	where
		K: AsRef<str>,
		V: AsRef<str>,
	{
		let key_upper = key.as_ref().to_uppercase();
		let new_line = format!("{}={}", key_upper, value.as_ref());
		let mut key_found = false;

		let content = fs::read_to_string(&self.env_path)?;
		let lines: Vec<String> = content
			.lines()
			.map(|line| {
				if let Some((k, _)) = line.split_once('=') {
					if k.trim().eq_ignore_ascii_case(&key_upper) {
						key_found = true;
						return new_line.clone();
					}
				}
				line.to_string()
			})
			.collect();

		let mut final_content = lines.join("\n");
		if !key_found {
			if !final_content.is_empty() {
				final_content.push('\n');
			}
			final_content.push_str(&new_line);
		}

		// Ensure file ends with a newline if not empty
		if !final_content.is_empty() && !final_content.ends_with('\n') {
			final_content.push('\n');
		}

		fs::write(&self.env_path, final_content)
	}

	/// Unsets (removes) a key from the .env file.
	///
	/// This function reads the file and writes it back without any lines
	/// that match the specified key. The key match is case-insensitive.
	pub fn unset<K>(&mut self, key: K) -> io::Result<()>
	where
		K: AsRef<str>,
	{
		let key_upper = key.as_ref().to_uppercase();

		let content = fs::read_to_string(&self.env_path)?;
		let lines: Vec<&str> = content
			.lines()
			.filter(|line| {
				if let Some((k, _)) = line.split_once('=') {
					// Keep the line if the key does NOT match
					!k.trim().eq_ignore_ascii_case(&key_upper)
				} else {
					// Keep lines without an '=' (e.g., blank lines)
					true
				}
			})
			.collect();

		let final_content = lines.join("\n");

		// Ensure file ends with a newline if not empty
		let final_content_with_newline = if final_content.is_empty() {
			final_content
		} else {
			format!("{}\n", final_content)
		};

		fs::write(&self.env_path, final_content_with_newline)
	}

	/// Returns a reference to the path of the temporary directory.
	pub fn path(&self) -> &Path {
		self.fs.path()
	}

	/// Returns a reference to the full path of the created .env file.
	pub fn env_path(&self) -> &Path {
		&self.env_path
	}

	/// Explicitly consumes the manager and deletes the temporary environment.
	pub fn cleanup(self) -> io::Result<()> {
		self.fs.cleanup()
	}
}
