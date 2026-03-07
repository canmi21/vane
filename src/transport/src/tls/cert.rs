use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

#[derive(Debug, thiserror::Error)]
pub enum CertError {
	#[error("no certificates found in PEM data")]
	NoCertificates,
	#[error("no private key found in PEM data")]
	NoPrivateKey,
	#[error("failed to parse certificate PEM")]
	InvalidCertPem(#[source] io::Error),
	#[error("failed to parse private key PEM")]
	InvalidKeyPem(#[source] io::Error),
	#[error("unsupported private key format")]
	UnsupportedKeyFormat(#[source] rustls::Error),
}

#[derive(Debug)]
pub struct LoadedCert {
	certs: Vec<CertificateDer<'static>>,
	key: PrivateKeyDer<'static>,
}

impl Clone for LoadedCert {
	fn clone(&self) -> Self {
		Self { certs: self.certs.clone(), key: self.key.clone_key() }
	}
}

impl LoadedCert {
	pub fn certs(&self) -> &[CertificateDer<'static>] {
		&self.certs
	}

	pub const fn key(&self) -> &PrivateKeyDer<'static> {
		&self.key
	}

	pub fn key_clone(&self) -> PrivateKeyDer<'static> {
		self.key.clone_key()
	}
}

/// Parse PEM-encoded certificate chain and private key into a [`LoadedCert`].
///
/// Supports PKCS8, PKCS1, and SEC1 private key formats (auto-detected by `rustls-pemfile`).
pub fn parse_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<LoadedCert, CertError> {
	let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut io::Cursor::new(cert_pem))
		.collect::<Result<Vec<_>, _>>()
		.map_err(CertError::InvalidCertPem)?;

	if certs.is_empty() {
		return Err(CertError::NoCertificates);
	}

	let key = rustls_pemfile::private_key(&mut io::Cursor::new(key_pem))
		.map_err(CertError::InvalidKeyPem)?
		.ok_or(CertError::NoPrivateKey)?;

	// Validate the key format is usable
	any_supported_type(&key).map_err(CertError::UnsupportedKeyFormat)?;

	Ok(LoadedCert { certs, key })
}

#[derive(Debug, Default)]
pub struct CertStore {
	certs: HashMap<String, Arc<LoadedCert>>,
}

impl CertStore {
	#[must_use]
	pub fn new() -> Self {
		Self::default()
	}

	pub fn insert(&mut self, name: impl Into<String>, cert: LoadedCert) {
		self.certs.insert(name.into(), Arc::new(cert));
	}

	/// Look up a cert by name, falling back to `"default"` if not found.
	#[must_use]
	pub fn get(&self, name: &str) -> Option<&Arc<LoadedCert>> {
		self.certs.get(name).or_else(|| self.certs.get("default"))
	}

	pub fn remove(&mut self, name: &str) -> Option<Arc<LoadedCert>> {
		self.certs.remove(name)
	}

	#[must_use]
	pub fn len(&self) -> usize {
		self.certs.len()
	}

	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.certs.is_empty()
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
	use super::*;

	fn generate_self_signed() -> (Vec<u8>, Vec<u8>) {
		let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
		let cert_pem = cert.cert.pem().into_bytes();
		let key_pem = cert.key_pair.serialize_pem().into_bytes();
		(cert_pem, key_pem)
	}

	#[test]
	fn parse_valid_self_signed() {
		let (cert_pem, key_pem) = generate_self_signed();
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();
		assert_eq!(loaded.certs().len(), 1);
	}

	#[test]
	fn parse_empty_cert_pem() {
		let (_, key_pem) = generate_self_signed();
		let err = parse_pem(b"", &key_pem).unwrap_err();
		assert!(matches!(err, CertError::NoCertificates));
	}

	#[test]
	fn parse_empty_key_pem() {
		let (cert_pem, _) = generate_self_signed();
		let err = parse_pem(&cert_pem, b"").unwrap_err();
		assert!(matches!(err, CertError::NoPrivateKey));
	}

	#[test]
	fn parse_garbage_cert_pem() {
		let (_, key_pem) = generate_self_signed();
		let err = parse_pem(b"not a cert", &key_pem).unwrap_err();
		assert!(matches!(err, CertError::NoCertificates));
	}

	#[test]
	fn parse_garbage_key_pem() {
		let (cert_pem, _) = generate_self_signed();
		let err = parse_pem(&cert_pem, b"not a key").unwrap_err();
		assert!(matches!(err, CertError::NoPrivateKey));
	}

	#[test]
	fn loaded_cert_clone() {
		let (cert_pem, key_pem) = generate_self_signed();
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();
		let cloned = loaded.clone();
		assert_eq!(loaded.certs().len(), cloned.certs().len());
	}

	#[test]
	fn store_insert_and_get() {
		let (cert_pem, key_pem) = generate_self_signed();
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();
		let mut store = CertStore::new();
		store.insert("my-cert", loaded);
		assert!(store.get("my-cert").is_some());
		assert_eq!(store.len(), 1);
	}

	#[test]
	fn store_fallback_to_default() {
		let (cert_pem, key_pem) = generate_self_signed();
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();
		let mut store = CertStore::new();
		store.insert("default", loaded);
		// Looking up a non-existent name falls back to "default"
		assert!(store.get("unknown").is_some());
	}

	#[test]
	fn store_missing_no_default() {
		let store = CertStore::new();
		assert!(store.get("anything").is_none());
	}

	#[test]
	fn store_remove() {
		let (cert_pem, key_pem) = generate_self_signed();
		let loaded = parse_pem(&cert_pem, &key_pem).unwrap();
		let mut store = CertStore::new();
		store.insert("removable", loaded);
		assert!(store.remove("removable").is_some());
		assert!(store.get("removable").is_none());
		assert!(store.is_empty());
	}
}
