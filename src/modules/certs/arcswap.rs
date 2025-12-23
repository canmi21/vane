/* src/modules/certs/arcswap.rs */

use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::{
	CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
};

// Clone derive (PrivateKeyDer is not implicitly Clone)
#[derive(Debug)]
pub struct LoadedCert {
	pub certs: Vec<CertificateDer<'static>>,
	pub key: PrivateKeyDer<'static>,
}

impl LoadedCert {
	/// Manually clones the PrivateKeyDer.
	/// PrivateKeyDer doesn't implement Clone to prevent accidental copying of secrets,
	/// so we extract the raw bytes and construct a new instance.
	pub fn key_clone(&self) -> PrivateKeyDer<'static> {
		match &self.key {
			PrivateKeyDer::Pkcs8(k) => {
				PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(k.secret_pkcs8_der().to_vec()))
			}
			PrivateKeyDer::Pkcs1(k) => {
				PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(k.secret_pkcs1_der().to_vec()))
			}
			PrivateKeyDer::Sec1(k) => {
				PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(k.secret_sec1_der().to_vec()))
			}
			_ => panic!("Unsupported key format in registry"),
		}
	}
}

pub static CERT_REGISTRY: Lazy<ArcSwap<HashMap<String, Arc<LoadedCert>>>> =
	Lazy::new(|| ArcSwap::from_pointee(HashMap::new()));

pub fn update_registry(new_map: HashMap<String, Arc<LoadedCert>>) {
	CERT_REGISTRY.store(Arc::new(new_map));
}

/// Retrieves a certificate by its ID. Returns an Arc (cheap clone).
pub fn get_certificate(id: &str) -> Option<Arc<LoadedCert>> {
	CERT_REGISTRY.load().get(id).cloned()
}
