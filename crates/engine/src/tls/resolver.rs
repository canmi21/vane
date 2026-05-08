//! `VaneCertResolver`: rustls's [`rustls::server::ResolvesServerCert`]
//! implementation backed by an `ArcSwap<CertStore>`. Reads the current
//! store on every handshake — a populator-driven swap is observed by
//! the next `ClientHello`, never mid-connection (TLS does not permit
//! that).
//!
//! The resolver shell lives in [`rustls_sni_resolver`]; this module
//! exports the vane-specific type alias `VaneCertResolver = rustls_sni_resolver::Resolver<CertEntry>`.
//!
//! We do **not** delegate to rustls's built-in
//! `rustls::server::ResolvesServerCertUsingSni` because it returns
//! `None` (handshake failure) on unmatched SNI with no built-in
//! fallback hook; spec spec/crates/engine-tls.md § _Cert resolver_
//! requires `CertStore::default` as the explicit no-SNI fallback.

use crate::tls::CertEntry;

pub type VaneCertResolver = rustls_sni_resolver::Resolver<CertEntry>;

#[cfg(test)]
mod tests {
	use std::collections::HashMap;
	use std::sync::Arc;
	use std::time::{Duration, SystemTime};

	use arc_swap::ArcSwap;
	use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
	use rustls::sign::CertifiedKey;

	use super::*;
	use crate::tls::CertStore;

	fn install_crypto() {
		crate::crypto::install_default_provider();
	}

	fn entry_for(host: &str) -> Arc<CertEntry> {
		install_crypto();
		let issued =
			rcgen::generate_simple_self_signed(vec![host.to_owned()]).expect("self-signed cert");
		let cert_der = CertificateDer::from(issued.cert.der().to_vec());
		let key_der = PrivatePkcs8KeyDer::from(issued.signing_key.serialize_der());
		let signing = rustls::crypto::CryptoProvider::get_default()
			.expect("crypto provider")
			.key_provider
			.load_private_key(rustls::pki_types::PrivateKeyDer::Pkcs8(key_der))
			.expect("load_private_key");
		let key = Arc::new(CertifiedKey::new(vec![cert_der], signing));
		Arc::new(CertEntry {
			key,
			not_after: SystemTime::now() + Duration::from_hours(1),
			ocsp_next_update: None,
		})
	}

	#[test]
	fn arcswap_store_visible_to_subsequent_lookup() {
		let api = entry_for("api.example.com");
		let mut by_sni = HashMap::new();
		by_sni.insert("api.example.com".to_owned(), Arc::clone(&api));
		let initial = CertStore { by_sni, default: None };
		let arcswap = Arc::new(ArcSwap::from_pointee(initial));
		// Hot lookup hits the api entry.
		assert!(Arc::ptr_eq(&arcswap.load().lookup(Some("api.example.com")).expect("hit"), &api.key));
		// Replace with a store whose SNI map only contains a fresh entry.
		let admin = entry_for("admin.example.com");
		let mut by_sni = HashMap::new();
		by_sni.insert("admin.example.com".to_owned(), Arc::clone(&admin));
		arcswap.store(Arc::new(CertStore { by_sni, default: None }));
		// The previously-resolving SNI is gone; the fresh one resolves.
		assert!(arcswap.load().lookup(Some("api.example.com")).is_none());
		assert!(Arc::ptr_eq(
			&arcswap.load().lookup(Some("admin.example.com")).expect("hit fresh"),
			&admin.key,
		));
	}

	#[test]
	fn resolver_constructible_from_arcswap() {
		// VaneCertResolver::resolve takes a `rustls::ClientHello`, which
		// has no public constructor; the e2e tests in
		// `crates/engine/tests/listener_tls.rs` exercise the live SNI
		// path. Here we cover construction and the trait wiring.
		let store =
			Arc::new(ArcSwap::from_pointee(CertStore { by_sni: HashMap::new(), default: None }));
		let _resolver: Arc<dyn rustls::server::ResolvesServerCert> =
			Arc::new(VaneCertResolver::new(store));
	}
}
