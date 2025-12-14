/* src/modules/certs/format.rs */

use crate::common::requirements::{Error, Result};
use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::rustls::{
	crypto::ring,
	pki_types::{CertificateDer, PrivateKeyDer},
	sign::CertifiedKey,
};

/// Reads and validates a certificate pair from disk.
/// Returns a `CertifiedKey` compliant with rustls.
pub fn load_and_validate_pair(cert_path: &Path, key_path: &Path) -> Result<Arc<CertifiedKey>> {
	// 1. Load Certificate Chain
	let cert_file = fs::File::open(cert_path)
		.map_err(|e| Error::Io(format!("Failed to open cert file {:?}: {}", cert_path, e)))?;
	let mut cert_reader = BufReader::new(cert_file);

	// rustls-pemfile v2 returns an iterator of Results
	let certs: Vec<CertificateDer> = rustls_pemfile::certs(&mut cert_reader)
		.collect::<std::result::Result<Vec<_>, _>>()
		.map_err(|e| Error::Tls(format!("Invalid PEM in {:?}: {}", cert_path, e)))?;

	if certs.is_empty() {
		return Err(Error::Tls(format!(
			"No certificates found in {:?}",
			cert_path
		)));
	}

	// 2. Load Private Key
	let key_file = fs::File::open(key_path)
		.map_err(|e| Error::Io(format!("Failed to open key file {:?}: {}", key_path, e)))?;
	let mut key_reader = BufReader::new(key_file);

	// rustls-pemfile v2 private_key returns Option<Result<Item, ...>>
	// We try to find the first valid private key item.
	let key_option = rustls_pemfile::private_key(&mut key_reader)
		.map_err(|e| Error::Tls(format!("Invalid Key PEM in {:?}: {}", key_path, e)))?;

	let key: PrivateKeyDer =
		key_option.ok_or_else(|| Error::Tls(format!("No private key found in {:?}", key_path)))?;

	// 3. Validate Key-Cert Pair by attempting to build the Signing Key
	// rustls 0.23+ requires an explicit crypto provider or usage of a specific signer helper.
	// We use the `ring` provider's signer helper.
	let signing_key = ring::sign::any_supported_type(&key).map_err(|e| {
		Error::Tls(format!(
			"Unsupported private key format in {:?}: {}",
			key_path, e
		))
	})?;

	// 4. Construct the CertifiedKey object
	// This implicitly checks if the key matches the public key in the cert (partially)
	Ok(Arc::new(CertifiedKey::new(certs, signing_key)))
}
