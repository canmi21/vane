/* src/primitives/src/certs/arcswap.rs */

use live::holder::{Store, UnloadPolicy};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio_rustls::rustls::pki_types::{
	CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
};

use crate::common::sys::lifecycle::{Error, Result};

// PrivateKeyDer is not implicitly Clone
#[derive(Debug)]
pub struct LoadedCert {
	pub certs: Vec<CertificateDer<'static>>,
	pub key: PrivateKeyDer<'static>,
}

impl Clone for LoadedCert {
	fn clone(&self) -> Self {
		Self {
			certs: self.certs.clone(),
			key: self.key_clone().expect("failed to clone key in LoadedCert::clone"),
		}
	}
}

impl LoadedCert {
	/// Manually clones the PrivateKeyDer.
	pub fn key_clone(&self) -> Result<PrivateKeyDer<'static>> {
		match &self.key {
			PrivateKeyDer::Pkcs8(k) => {
				Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(k.secret_pkcs8_der().to_vec())))
			}
			PrivateKeyDer::Pkcs1(k) => {
				Ok(PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(k.secret_pkcs1_der().to_vec())))
			}
			PrivateKeyDer::Sec1(k) => {
				Ok(PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(k.secret_sec1_der().to_vec())))
			}
			_ => Err(Error::Tls("Unsupported key format in registry".into())),
		}
	}
}

pub static CERT_REGISTRY: LazyLock<Store<LoadedCert>> = LazyLock::new(Store::new);

pub fn update_registry(new_map: HashMap<String, Arc<LoadedCert>>) {
	for (id, cert) in new_map {
		CERT_REGISTRY.insert(
			id,
			(*cert).clone(),
			std::path::PathBuf::from("memory"),
			UnloadPolicy::Removable,
		);
	}
}

/// Retrieves a certificate by its ID. Returns an Arc (cheap clone).
pub fn get_certificate(id: &str) -> Option<Arc<LoadedCert>> {
	CERT_REGISTRY.get(id)
}
