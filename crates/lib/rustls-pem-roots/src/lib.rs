//! Load PEM-encoded CA certificates from any combination of explicit
//! files and a roots directory into a `rustls::RootCertStore`,
//! deduplicating identical certificates by full DER bytes and
//! distinguishing "path unreadable" / "PEM unparseable" / "no roots
//! found" as separate error variants.
//!
//! `rustls-pemfile` only reaches the single-file parse layer; this
//! crate is the small "drop a folder of CA files in here" loader that
//! every operator-facing TLS service ends up writing by hand.

use std::collections::HashSet;
use std::fs;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};

use rustls::RootCertStore;

/// Load failure. Variants name the file or directory that failed and
/// carry the underlying `io::Error` where applicable.
#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("trust store is empty after loading {files:?} + {dir:?}")]
	Empty { files: Vec<PathBuf>, dir: Option<PathBuf> },

	#[error("read file {path:?}: {source}")]
	ReadFile { path: PathBuf, source: std::io::Error },

	#[error("parse file {path:?}: {source}")]
	ParseFile { path: PathBuf, source: std::io::Error },

	#[error("file {path:?} has no certs")]
	EmptyFile { path: PathBuf },

	#[error("read dir {dir:?}: {source}")]
	ReadDir { dir: PathBuf, source: std::io::Error },
}

/// Build a fresh `RootCertStore` from a list of explicit PEM files and
/// an optional directory of `*.pem` files. Certificates are added with
/// [`RootCertStore::add`] and deduplicated by their DER bytes, so a
/// cert that appears in both a file and the directory contributes only
/// one entry to the store.
///
/// Returns [`Error::Empty`] if no certificates are loaded.
///
/// # Errors
///
/// Surfaces the variant of [`Error`] naming the file or directory that
/// failed to load or parse.
pub fn load(files: &[PathBuf], dir: Option<&Path>) -> Result<RootCertStore, Error> {
	let mut roots = RootCertStore::empty();
	let mut seen: HashSet<Vec<u8>> = HashSet::new();

	for path in files {
		add_pem_file(path, &mut roots, &mut seen)?;
	}
	if let Some(d) = dir {
		add_pem_dir(d, &mut roots, &mut seen)?;
	}

	if roots.is_empty() {
		return Err(Error::Empty { files: files.to_vec(), dir: dir.map(Path::to_path_buf) });
	}
	Ok(roots)
}

/// Add every CA certificate from one PEM file to `roots`, skipping any
/// whose DER bytes already appear in `seen`.
///
/// Useful when composing multiple sources into one store and the
/// caller wants to track dedup state across calls. [`Error::EmptyFile`]
/// fires only when the file has no parseable PEM certs at all — a file
/// whose every cert is already in `seen` (a legitimate dedup) is not
/// an error.
///
/// # Errors
///
/// Surfaces [`Error::ReadFile`], [`Error::ParseFile`], or
/// [`Error::EmptyFile`] for the named file.
pub fn add_pem_file<S: BuildHasher>(
	path: &Path,
	roots: &mut RootCertStore,
	seen: &mut HashSet<Vec<u8>, S>,
) -> Result<(), Error> {
	let bytes =
		fs::read(path).map_err(|source| Error::ReadFile { path: path.to_path_buf(), source })?;
	let mut reader = std::io::BufReader::new(bytes.as_slice());
	let mut parsed = 0_usize;
	for cert in rustls_pemfile::certs(&mut reader) {
		let cert = cert.map_err(|source| Error::ParseFile { path: path.to_path_buf(), source })?;
		parsed += 1;
		let der_bytes = cert.as_ref().to_vec();
		if seen.insert(der_bytes)
			&& let Err(e) = roots.add(cert)
		{
			return Err(Error::ParseFile {
				path: path.to_path_buf(),
				source: std::io::Error::other(e.to_string()),
			});
		}
	}
	if parsed == 0 {
		return Err(Error::EmptyFile { path: path.to_path_buf() });
	}
	Ok(())
}

/// Add every `*.pem` file from `dir` to `roots`, dedup-by-DER through
/// `seen`. Non-`.pem` entries are silently skipped; subdirectories are
/// not recursed.
///
/// # Errors
///
/// Surfaces [`Error::ReadDir`] for the directory itself, or any
/// per-file variant from [`add_pem_file`].
pub fn add_pem_dir<S: BuildHasher>(
	dir: &Path,
	roots: &mut RootCertStore,
	seen: &mut HashSet<Vec<u8>, S>,
) -> Result<(), Error> {
	let entries =
		fs::read_dir(dir).map_err(|source| Error::ReadDir { dir: dir.to_path_buf(), source })?;
	for entry in entries {
		let entry = entry.map_err(|source| Error::ReadDir { dir: dir.to_path_buf(), source })?;
		let path = entry.path();
		if path.extension().and_then(|s| s.to_str()) != Some("pem") {
			continue;
		}
		add_pem_file(&path, roots, seen)?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use rcgen::{CertificateParams, KeyPair};
	use std::io::Write;
	use tempfile::TempDir;

	fn write_self_signed(path: &Path, cn: &str) {
		let mut params = CertificateParams::new(vec![cn.to_owned()]).unwrap();
		params.distinguished_name.push(rcgen::DnType::CommonName, cn);
		let key = KeyPair::generate().unwrap();
		let cert = params.self_signed(&key).unwrap();
		let mut f = fs::File::create(path).unwrap();
		f.write_all(cert.pem().as_bytes()).unwrap();
	}

	#[test]
	fn load_files_only() {
		let tmp = TempDir::new().unwrap();
		let p = tmp.path().join("a.pem");
		write_self_signed(&p, "ca-a");
		let store = load(std::slice::from_ref(&p), None).unwrap();
		assert!(!store.is_empty());
	}

	#[test]
	fn load_dir_only() {
		let tmp = TempDir::new().unwrap();
		write_self_signed(&tmp.path().join("a.pem"), "ca-a");
		write_self_signed(&tmp.path().join("b.pem"), "ca-b");
		let store = load(&[], Some(tmp.path())).unwrap();
		assert_eq!(store.len(), 2);
	}

	#[test]
	fn load_dir_skips_non_pem() {
		let tmp = TempDir::new().unwrap();
		write_self_signed(&tmp.path().join("a.pem"), "ca-a");
		fs::write(tmp.path().join("notes.txt"), b"not a pem").unwrap();
		let store = load(&[], Some(tmp.path())).unwrap();
		assert_eq!(store.len(), 1);
	}

	#[test]
	fn load_dedupes_across_file_and_dir() {
		let tmp = TempDir::new().unwrap();
		let f = tmp.path().join("a.pem");
		write_self_signed(&f, "ca-a");
		// Same file referenced by both an explicit path and the dir
		// scan; should result in exactly 1 cert in the store.
		let store = load(&[f], Some(tmp.path())).unwrap();
		assert_eq!(store.len(), 1);
	}

	#[test]
	fn load_empty_returns_empty_error() {
		let tmp = TempDir::new().unwrap();
		let err = load(&[], Some(tmp.path())).unwrap_err();
		assert!(matches!(err, Error::Empty { .. }));
	}

	#[test]
	fn load_missing_file_returns_read_file_error() {
		let tmp = TempDir::new().unwrap();
		let missing = tmp.path().join("nope.pem");
		let err = load(&[missing], None).unwrap_err();
		assert!(matches!(err, Error::ReadFile { .. }));
	}

	#[test]
	fn load_empty_pem_returns_empty_file_error() {
		let tmp = TempDir::new().unwrap();
		let p = tmp.path().join("empty.pem");
		fs::write(&p, b"").unwrap();
		let err = load(&[p], None).unwrap_err();
		assert!(matches!(err, Error::EmptyFile { .. }));
	}

	#[test]
	fn load_missing_dir_returns_read_dir_error() {
		let tmp = TempDir::new().unwrap();
		let missing = tmp.path().join("nodir");
		let err = load(&[], Some(&missing)).unwrap_err();
		assert!(matches!(err, Error::ReadDir { .. }));
	}
}
