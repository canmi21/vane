//! `CertStore` and `CertEntry`: the in-memory cert pool a
//! [`crate::tls::VaneCertResolver`] hands to rustls during handshake.
//!
//! Keys in [`CertStore::by_sni`] are stored ASCII-lowercase per the SNI
//! normalization invariant (spec/crates/engine-tls.md § _SNI peek (L4, no decrypt)_),
//! so resolver-side lookups are byte-for-byte without an
//! `eq_ignore_ascii_case` shim.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

/// One cert + key bundle, plus the metadata a refresh scheduler uses to
/// decide whether the entry is stale. `key` is the rustls handshake
/// material (cert chain + signing key + optional OCSP staple).
#[derive(Debug)]
pub struct CertEntry {
	pub key: Arc<rustls::sign::CertifiedKey>,
	/// Leaf cert's `notAfter`. Populators parse it from the cert chain
	/// so the refresh scheduler can compare against `SystemTime::now()`
	/// without re-decoding x509 every tick.
	pub not_after: SystemTime,
	/// `nextUpdate` of the staple in `key.ocsp`, if any. Populators
	/// that don't fetch OCSP (e.g. [`crate::tls::StaticCertPopulator`])
	/// always set this to `None`.
	pub ocsp_next_update: Option<Instant>,
}

/// Per-listener cert pool: zero-or-more SNI-keyed entries plus an
/// optional sni-less default. The default fires when a `ClientHello`
/// has no SNI extension or when the SNI doesn't match any
/// [`Self::by_sni`] key. A listener has at most one default.
#[derive(Debug)]
pub struct CertStore {
	pub by_sni: HashMap<String, Arc<CertEntry>>,
	pub default: Option<Arc<CertEntry>>,
}

impl CertStore {
	/// Resolve a `ClientHello`'s SNI against the store. The hot-path
	/// resolver delegates to this so unit tests can exercise the
	/// lookup without constructing a `rustls::ClientHello` (which is
	/// not user-constructible). `sni` is expected to already be
	/// ASCII-lowercased by rustls per RFC 6066 § 3.
	#[must_use]
	pub fn lookup(&self, sni: Option<&str>) -> Option<Arc<rustls::sign::CertifiedKey>> {
		if let Some(name) = sni
			&& let Some(entry) = self.by_sni.get(name)
		{
			return Some(Arc::clone(&entry.key));
		}
		self.default.as_ref().map(|d| Arc::clone(&d.key))
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
	use rustls::sign::CertifiedKey;

	use super::*;

	fn install_crypto() {
		crate::crypto::install_default_provider();
	}

	fn make_entry(host: &str) -> Arc<CertEntry> {
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
	fn lookup_hit_returns_keyed_entry() {
		let entry = make_entry("api.example.com");
		let mut by_sni = HashMap::new();
		by_sni.insert("api.example.com".to_owned(), Arc::clone(&entry));
		let store = CertStore { by_sni, default: None };
		let got = store.lookup(Some("api.example.com")).expect("hit");
		assert!(Arc::ptr_eq(&got, &entry.key));
	}

	#[test]
	fn lookup_miss_falls_back_to_default() {
		let api = make_entry("api.example.com");
		let default = make_entry("default.example.com");
		let mut by_sni = HashMap::new();
		by_sni.insert("api.example.com".to_owned(), api);
		let store = CertStore { by_sni, default: Some(Arc::clone(&default)) };
		let got = store.lookup(Some("unknown.example.com")).expect("default fires");
		assert!(Arc::ptr_eq(&got, &default.key));
	}

	#[test]
	fn lookup_miss_with_no_default_returns_none() {
		let api = make_entry("api.example.com");
		let mut by_sni = HashMap::new();
		by_sni.insert("api.example.com".to_owned(), api);
		let store = CertStore { by_sni, default: None };
		assert!(store.lookup(Some("unknown.example.com")).is_none());
		assert!(store.lookup(None).is_none());
	}
}
