//! A minimal `ResolvesServerCert` implementation backed by
//! `{ by_sni: HashMap<String, Arc<E>>, default: Option<Arc<E>> }`,
//! with the whole struct designed to live behind an `Arc<ArcSwap<_>>`
//! so a config reload is one atomic pointer swap.
//!
//! `E` is generic over a [`EntryKey`] trait, so callers can attach
//! their own per-cert state (expiry timestamps, OCSP staple handles,
//! ACME order IDs) without a fork.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

/// A trait satisfied by anything that carries a rustls
/// `Arc<CertifiedKey>` (cert chain + signing key + optional OCSP
/// staple). Used by [`CertStore::lookup`] to extract the handshake
/// material from a caller-defined entry type.
pub trait EntryKey {
	fn key(&self) -> Arc<rustls::sign::CertifiedKey>;
}

/// Per-listener cert pool: zero-or-more SNI-keyed entries plus an
/// optional sni-less default. The default fires when a `ClientHello`
/// has no SNI extension or when the SNI does not match any
/// [`Self::by_sni`] key. A listener has at most one default.
///
/// Keys in [`Self::by_sni`] are stored ASCII-lowercase per RFC 6066
/// § 3 (`server_name` is already ASCII-lowercased by rustls), so
/// resolver-side lookups are byte-for-byte without an
/// `eq_ignore_ascii_case` shim.
#[derive(Debug)]
pub struct CertStore<E: EntryKey> {
	pub by_sni: HashMap<String, Arc<E>>,
	pub default: Option<Arc<E>>,
}

impl<E: EntryKey> CertStore<E> {
	#[must_use]
	pub fn new() -> Self {
		Self { by_sni: HashMap::new(), default: None }
	}

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
			return Some(entry.key());
		}
		self.default.as_ref().map(|d| d.key())
	}
}

impl<E: EntryKey> Default for CertStore<E> {
	fn default() -> Self {
		Self::new()
	}
}

/// `rustls::server::ResolvesServerCert` implementation backed by an
/// `ArcSwap<CertStore<E>>`. Reads the current store on every
/// handshake — a populator-driven swap is observed by the next
/// `ClientHello`, never mid-connection (TLS does not permit that).
///
/// We do **not** delegate to rustls's built-in
/// `rustls::server::ResolvesServerCertUsingSni` because it returns
/// `None` (handshake failure) on unmatched SNI with no built-in
/// fallback hook; this resolver uses [`CertStore::default`] as the
/// explicit no-SNI fallback.
#[derive(Debug)]
pub struct Resolver<E: EntryKey> {
	store: Arc<ArcSwap<CertStore<E>>>,
}

impl<E: EntryKey> Resolver<E> {
	#[must_use]
	pub fn new(store: Arc<ArcSwap<CertStore<E>>>) -> Self {
		Self { store }
	}
}

impl<E: EntryKey + std::fmt::Debug + Send + Sync + 'static> rustls::server::ResolvesServerCert
	for Resolver<E>
{
	fn resolve(
		&self,
		hello: rustls::server::ClientHello<'_>,
	) -> Option<Arc<rustls::sign::CertifiedKey>> {
		// `server_name()` is already ASCII-lowercased by rustls per
		// RFC 6066 § 3, so a direct map lookup suffices.
		self.store.load().lookup(hello.server_name())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
	use rustls::sign::CertifiedKey;

	#[derive(Debug)]
	struct TestEntry {
		key: Arc<CertifiedKey>,
	}

	impl EntryKey for TestEntry {
		fn key(&self) -> Arc<CertifiedKey> {
			Arc::clone(&self.key)
		}
	}

	fn install_crypto() {
		let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
	}

	fn make_entry(host: &str) -> Arc<TestEntry> {
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
		Arc::new(TestEntry { key })
	}

	#[test]
	fn lookup_hit_returns_keyed_entry() {
		let entry = make_entry("api.example.com");
		let mut store: CertStore<TestEntry> = CertStore::new();
		store.by_sni.insert("api.example.com".to_owned(), Arc::clone(&entry));
		let got = store.lookup(Some("api.example.com")).expect("hit");
		assert!(Arc::ptr_eq(&got, &entry.key));
	}

	#[test]
	fn lookup_miss_falls_back_to_default() {
		let api = make_entry("api.example.com");
		let default = make_entry("default.example.com");
		let mut store: CertStore<TestEntry> = CertStore::new();
		store.by_sni.insert("api.example.com".to_owned(), api);
		store.default = Some(Arc::clone(&default));
		let got = store.lookup(Some("unknown.example.com")).expect("default fires");
		assert!(Arc::ptr_eq(&got, &default.key));
	}

	#[test]
	fn lookup_miss_with_no_default_returns_none() {
		let api = make_entry("api.example.com");
		let mut store: CertStore<TestEntry> = CertStore::new();
		store.by_sni.insert("api.example.com".to_owned(), api);
		assert!(store.lookup(Some("unknown.example.com")).is_none());
		assert!(store.lookup(None).is_none());
	}

	#[test]
	fn lookup_no_sni_uses_default() {
		let default = make_entry("default.example.com");
		let mut store: CertStore<TestEntry> = CertStore::new();
		store.default = Some(Arc::clone(&default));
		let got = store.lookup(None).expect("default fires");
		assert!(Arc::ptr_eq(&got, &default.key));
	}

	#[test]
	fn arcswap_store_visible_to_subsequent_lookup() {
		let api = make_entry("api.example.com");
		let mut initial: CertStore<TestEntry> = CertStore::new();
		initial.by_sni.insert("api.example.com".to_owned(), Arc::clone(&api));
		let arcswap = Arc::new(ArcSwap::from_pointee(initial));

		assert!(Arc::ptr_eq(&arcswap.load().lookup(Some("api.example.com")).expect("hit"), &api.key));

		let admin = make_entry("admin.example.com");
		let mut fresh: CertStore<TestEntry> = CertStore::new();
		fresh.by_sni.insert("admin.example.com".to_owned(), Arc::clone(&admin));
		arcswap.store(Arc::new(fresh));

		assert!(arcswap.load().lookup(Some("api.example.com")).is_none());
		assert!(Arc::ptr_eq(
			&arcswap.load().lookup(Some("admin.example.com")).expect("hit fresh"),
			&admin.key
		));
	}

	#[test]
	fn resolver_constructible_from_arcswap() {
		// Resolver::resolve takes a `rustls::ClientHello`, which has
		// no public constructor; downstream e2e tests exercise the
		// live SNI path. Here we cover construction and trait wiring.
		let store: Arc<ArcSwap<CertStore<TestEntry>>> =
			Arc::new(ArcSwap::from_pointee(CertStore::new()));
		let _resolver: Arc<dyn rustls::server::ResolvesServerCert> = Arc::new(Resolver::new(store));
	}
}
