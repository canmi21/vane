/* src/modules/certs/arcswap.rs */

use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_rustls::rustls::sign::CertifiedKey;

/// The global container for loaded certificates.
/// Map Key: The filename stem (e.g., "example" for "example.crt").
/// Map Value: The validated CertifiedKey ready for TLS handshakes.
pub static CERT_REGISTRY: Lazy<ArcSwap<HashMap<String, Arc<CertifiedKey>>>> =
	Lazy::new(|| ArcSwap::from_pointee(HashMap::new()));

/// Updates the global certificate registry.
pub fn update_registry(new_map: HashMap<String, Arc<CertifiedKey>>) {
	CERT_REGISTRY.store(Arc::new(new_map));
}

/// Retrieves a certificate by its ID (filename stem).
pub fn get_certificate(id: &str) -> Option<Arc<CertifiedKey>> {
	CERT_REGISTRY.load().get(id).cloned()
}
