/* src/resources/certs/format.rs */

use super::arcswap::LoadedCert;
use crate::common::sys::lifecycle::{Error, Result};
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub async fn load_and_validate_pair(cert_path: &Path, key_path: &Path) -> Result<Arc<LoadedCert>> {
	let cert_data = tokio::fs::read(cert_path)
		.await
		.map_err(|e| Error::Io(format!("Failed to read cert file {:?}: {}", cert_path, e)))?;
	let mut cert_cursor = Cursor::new(cert_data);

	let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_cursor)
		.collect::<std::result::Result<Vec<_>, _>>()
		.map_err(|e| Error::Tls(format!("Invalid PEM in {:?}: {}", cert_path, e)))?;

	if certs.is_empty() {
		return Err(Error::Tls(format!(
			"No certificates found in {:?}",
			cert_path
		)));
	}

	let key_data = tokio::fs::read(key_path)
		.await
		.map_err(|e| Error::Io(format!("Failed to read key file {:?}: {}", key_path, e)))?;
	let mut key_cursor = Cursor::new(key_data);

	let key_option = rustls_pemfile::private_key(&mut key_cursor)
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
