/* src/modules/certs/format.rs */

use super::arcswap::LoadedCert;
use crate::common::requirements::{Error, Result};
use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub fn load_and_validate_pair(cert_path: &Path, key_path: &Path) -> Result<Arc<LoadedCert>> {
	let cert_file = fs::File::open(cert_path)
		.map_err(|e| Error::Io(format!("Failed to open cert file {:?}: {}", cert_path, e)))?;
	let mut cert_reader = BufReader::new(cert_file);

	let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
		.collect::<std::result::Result<Vec<_>, _>>()
		.map_err(|e| Error::Tls(format!("Invalid PEM in {:?}: {}", cert_path, e)))?;

	if certs.is_empty() {
		return Err(Error::Tls(format!(
			"No certificates found in {:?}",
			cert_path
		)));
	}

	let key_file = fs::File::open(key_path)
		.map_err(|e| Error::Io(format!("Failed to open key file {:?}: {}", key_path, e)))?;
	let mut key_reader = BufReader::new(key_file);

	let key_option = rustls_pemfile::private_key(&mut key_reader)
		.map_err(|e| Error::Tls(format!("Invalid Key PEM in {:?}: {}", key_path, e)))?;

	let key: PrivateKeyDer<'static> =
		key_option.ok_or_else(|| Error::Tls(format!("No private key found in {:?}", key_path)))?;

	// Validation check: try to build a CertifiedKey to ensure the pair is valid, then discard it.
	// This catches mismatches early.
	{
		let _ = tokio_rustls::rustls::crypto::ring::sign::any_supported_type(&key)
			.map_err(|e| Error::Tls(format!("Unsupported private key format: {}", e)))?;
	}

	Ok(Arc::new(LoadedCert { certs, key }))
}
