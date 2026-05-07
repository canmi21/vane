//! `ManagedCertPopulator` — `impl CertPopulator` view over a
//! [`ManagedCertRegistry`]. Per `spec/acme.md` § _Architecture_ the
//! registry is daemon-scoped (one instance, lives across reloads);
//! the populator is `FlowGraph`-scoped (one per listener with managed
//! rules, rebuilt on every reload).
//!
//! [`ManagedCertPopulator::initial_store`] pulls whatever certs the
//! registry already has cached for the listener's declared SNI set;
//! missing SNIs are simply absent from the resulting [`CertStore`]
//! (handshakes for those SNIs fail at the resolver until issuance
//! catches up). [`ManagedCertPopulator::refresh`] re-pulls from the
//! registry and skips the `ArcSwap` install when the leaf-DER set
//! hasn't changed, so 5-minute ticks stay no-ops on steady state.
//!
//! Defaults (sni-less certs) are not managed by this populator — by
//! `spec/acme.md` § _Configuration schema_ a managed cert always
//! requires `tls.sni`, and the lower pass routes all sni-less specs
//! into [`vane_core::rule::ListenerTlsSpec::default`] which is
//! always static. Mixed listeners (static default + managed
//! per-SNI) are handled by stacking this populator alongside a
//! [`crate::tls::StaticCertPopulator`] in the listener's populator
//! list — the merge happens at `FlowGraph::link` time (see
//! `crates/engine/src/flow_graph.rs`).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::registry::ManagedCertRegistry;
use super::store::StoredCert;
use crate::tls::populator::{CertPopulator, PopulatorError};
use crate::tls::{CertEntry, CertStore};

/// FlowGraph-scoped `CertPopulator` that delivers ACME-issued certs
/// from the daemon-scoped [`ManagedCertRegistry`].
///
/// Holds a strong `Arc` to the registry so the populator can outlive
/// any single `FlowGraph` swap-out without keeping ACME state alive
/// past daemon shutdown — both ends are `Arc`s rooted in the daemon.
pub struct ManagedCertPopulator {
	registry: Arc<ManagedCertRegistry>,
	/// SNIs this listener wants managed. Sorted + deduped so two
	/// populators sharing a registry produce identical outputs for
	/// identical inputs (debug-stable, hash-stable for any future
	/// fingerprinting).
	snis: Vec<String>,
}

impl std::fmt::Debug for ManagedCertPopulator {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("ManagedCertPopulator").field("snis", &self.snis).finish_non_exhaustive()
	}
}

impl ManagedCertPopulator {
	/// Construct a populator for `snis` against `registry`. The list
	/// is normalised (lowercased + sorted + deduped); the registry is
	/// told via [`ManagedCertRegistry::declare_managed`] which SNIs
	/// this `FlowGraph` generation wants tracked.
	///
	/// The set of "needs first-time issuance" SNIs returned by
	/// `declare_managed` is intentionally discarded here — the daemon's
	/// boot hook (`crates/daemon/src/acme_boot.rs`) is the issuer of
	/// record. The populator is a passive view; issuance is kicked off
	/// elsewhere and surfaces here only when the registry's cache lands.
	pub fn new(registry: Arc<ManagedCertRegistry>, snis: Vec<String>) -> Self {
		let mut snis: Vec<String> = snis.into_iter().map(|s| s.to_ascii_lowercase()).collect();
		snis.sort();
		snis.dedup();
		let _ = registry.declare_managed(&snis);
		Self { registry, snis }
	}

	/// Build a fresh `CertStore` from the registry's current cache
	/// state. Shared between [`Self::initial_store`] and the inner
	/// branch of [`Self::refresh`] so they observe the same registry
	/// state per call (and, importantly, the same conversion errors).
	fn current_store(&self) -> Result<CertStore, PopulatorError> {
		let mut by_sni: HashMap<String, Arc<CertEntry>> = HashMap::with_capacity(self.snis.len());
		for sni in &self.snis {
			if let Some(stored) = self.registry.cert_for(sni) {
				by_sni.insert(sni.clone(), Arc::new(stored_to_cert_entry(&stored)?));
			}
			// Missing cert: skip the entry. Handshakes on this SNI
			// fail at the resolver until issuance lands.
		}
		Ok(CertStore { by_sni, default: None })
	}
}

#[async_trait]
impl CertPopulator for ManagedCertPopulator {
	async fn initial_store(&self) -> Result<CertStore, PopulatorError> {
		self.current_store()
	}

	async fn refresh(&self, current: &CertStore) -> Result<Option<CertStore>, PopulatorError> {
		let candidate = self.current_store()?;
		if cert_stores_equivalent(current, &candidate) { Ok(None) } else { Ok(Some(candidate)) }
	}
}

/// Convert a registry-tracked `StoredCert` (PEM + metadata) into a
/// rustls-ready [`CertEntry`]. Errors surface as
/// [`PopulatorError::Source`] with the SNI / step that failed; the
/// listener-side handler logs and skips the entry rather than failing
/// the whole refresh, but the caller decides — this fn just reports.
fn stored_to_cert_entry(stored: &StoredCert) -> Result<CertEntry, PopulatorError> {
	let mut full_pem = stored.leaf_pem.clone();
	if !stored.chain_pem.is_empty() {
		// `StoredCert.chain_pem` is the intermediates only (per
		// `acme/registry.rs::split_leaf_chain`); rustls expects the
		// full chain in the order leaf → intermediate(s) → root, so
		// we concatenate without re-parsing.
		full_pem.push_str(&stored.chain_pem);
	}

	let cert_chain: Vec<rustls::pki_types::CertificateDer<'static>> =
		rustls_pemfile::certs(&mut full_pem.as_bytes())
			.collect::<Result<_, _>>()
			.map_err(|e| PopulatorError::source(format!("parse stored cert chain: {e}")))?;
	if cert_chain.is_empty() {
		return Err(PopulatorError::source("stored cert chain contained no certificates"));
	}

	let private_key = rustls_pemfile::private_key(&mut stored.key_pem.as_bytes())
		.map_err(|e| PopulatorError::source(format!("parse stored key: {e}")))?
		.ok_or_else(|| PopulatorError::source("stored key contained no private key"))?;

	let provider = rustls::crypto::CryptoProvider::get_default()
		.ok_or_else(|| PopulatorError::source("rustls crypto provider not installed"))?;
	let signing_key = provider
		.key_provider
		.load_private_key(private_key)
		.map_err(|e| PopulatorError::source(format!("load_private_key: {e}")))?;

	Ok(CertEntry {
		key: Arc::new(rustls::sign::CertifiedKey::new(cert_chain, signing_key)),
		not_after: stored.not_after,
		ocsp_next_update: None,
	})
}

/// Structural equivalence check for two `CertStore`s: same SNI set
/// and same leaf DER bytes per SNI. Used by [`ManagedCertPopulator::refresh`]
/// to skip the `ArcSwap` install when nothing actually changed
/// (avoids spurious resolver rebuilds on every 5-minute tick when no
/// certs renewed).
///
/// Defaults are not compared because `ManagedCertPopulator` always
/// produces `default: None` — defaults are owned by the static
/// populator on a mixed listener.
fn cert_stores_equivalent(a: &CertStore, b: &CertStore) -> bool {
	if a.by_sni.len() != b.by_sni.len() {
		return false;
	}
	for (sni, ae) in &a.by_sni {
		let Some(be) = b.by_sni.get(sni) else { return false };
		if leaf_der(ae) != leaf_der(be) {
			return false;
		}
	}
	true
}

/// Leaf DER bytes off a `CertEntry`. Empty slice when the entry's
/// `CertifiedKey` has no cert chain (defensive — rustls rejects this
/// at handshake time, but we don't want to panic in the equality
/// check that runs on every refresh).
fn leaf_der(entry: &Arc<CertEntry>) -> &[u8] {
	entry.key.cert.first().map_or(&[][..], rustls::pki_types::CertificateDer::as_ref)
}

#[cfg(test)]
mod tests {
	use std::time::{Duration, SystemTime};

	use async_trait::async_trait;
	use parking_lot::Mutex;

	use super::*;
	use crate::acme::store::{AcmeAccount, AcmeStore, LockGuard, StoreError};

	#[derive(Default)]
	struct MockStore {
		accounts: Mutex<std::collections::BTreeMap<String, AcmeAccount>>,
		certs: Mutex<std::collections::BTreeMap<String, StoredCert>>,
	}

	#[derive(Debug)]
	struct MockGuard;
	impl LockGuard for MockGuard {}

	#[async_trait]
	impl AcmeStore for MockStore {
		async fn load_account(&self, dir: &str) -> Result<Option<AcmeAccount>, StoreError> {
			Ok(self.accounts.lock().get(dir).cloned())
		}
		async fn save_account(&self, dir: &str, acc: &AcmeAccount) -> Result<(), StoreError> {
			self.accounts.lock().insert(dir.to_owned(), acc.clone());
			Ok(())
		}
		async fn load_cert(&self, sni: &str) -> Result<Option<StoredCert>, StoreError> {
			Ok(self.certs.lock().get(sni).cloned())
		}
		async fn save_cert(&self, sni: &str, cert: &StoredCert) -> Result<(), StoreError> {
			self.certs.lock().insert(sni.to_owned(), cert.clone());
			Ok(())
		}
		async fn list_cert_snis(&self) -> Result<Vec<String>, StoreError> {
			let mut snis: Vec<String> = self.certs.lock().keys().cloned().collect();
			snis.sort();
			Ok(snis)
		}
		async fn lock(&self, _scope: &str) -> Result<Box<dyn LockGuard>, StoreError> {
			Ok(Box::new(MockGuard))
		}
	}

	/// Generate a real self-signed cert via `rcgen` so the populator's
	/// PEM-parse + `load_private_key` path is exercised end-to-end.
	/// Returns the `StoredCert` shape the registry would persist.
	fn make_stored_cert(sni: &str) -> StoredCert {
		crate::crypto::install_default_provider();
		let issued = rcgen::generate_simple_self_signed(vec![sni.to_owned()]).expect("self-signed");
		StoredCert {
			leaf_pem: issued.cert.pem(),
			chain_pem: String::new(),
			key_pem: issued.signing_key.serialize_pem(),
			not_after: SystemTime::now() + Duration::from_hours(24 * 30),
			ari_replacement_id: None,
			last_renew_at: SystemTime::now(),
		}
	}

	async fn registry_with(certs: &[(&str, StoredCert)]) -> Arc<ManagedCertRegistry> {
		let store = Arc::new(MockStore::default());
		for (sni, cert) in certs {
			store.save_cert(sni, cert).await.unwrap();
		}
		ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.expect("open")
	}

	#[tokio::test]
	async fn initial_store_pulls_cached_certs_only() {
		let registry = registry_with(&[("a.example", make_stored_cert("a.example"))]).await;
		let populator = ManagedCertPopulator::new(
			Arc::clone(&registry),
			vec!["a.example".into(), "b.example".into()],
		);
		let store = populator.initial_store().await.expect("initial_store");
		assert!(store.by_sni.contains_key("a.example"));
		assert!(!store.by_sni.contains_key("b.example"));
		assert!(store.default.is_none(), "managed populator never owns default");
	}

	#[tokio::test]
	async fn initial_store_lowercases_sni_input() {
		let registry = registry_with(&[("api.example", make_stored_cert("api.example"))]).await;
		let populator = ManagedCertPopulator::new(Arc::clone(&registry), vec!["API.EXAMPLE".into()]);
		let store = populator.initial_store().await.expect("initial_store");
		// Registry caches lowercased; populator must look up by the
		// same case to hit the cache.
		assert!(store.by_sni.contains_key("api.example"));
	}

	#[tokio::test]
	async fn refresh_returns_none_when_certs_unchanged() {
		let registry = registry_with(&[("x.example", make_stored_cert("x.example"))]).await;
		let populator = ManagedCertPopulator::new(Arc::clone(&registry), vec!["x.example".into()]);
		let store = populator.initial_store().await.expect("initial_store");
		let refreshed = populator.refresh(&store).await.expect("refresh");
		assert!(refreshed.is_none(), "no churn → no swap");
	}

	#[tokio::test]
	async fn refresh_returns_some_when_new_cert_lands() {
		let registry = registry_with(&[("a.example", make_stored_cert("a.example"))]).await;
		let populator = ManagedCertPopulator::new(
			Arc::clone(&registry),
			vec!["a.example".into(), "b.example".into()],
		);
		let store = populator.initial_store().await.expect("initial_store");
		assert_eq!(store.by_sni.len(), 1);

		// Simulate a successful issuance landing for b.example.
		registry.cache_cert_for_test("b.example", Arc::new(make_stored_cert("b.example")));
		let refreshed = populator.refresh(&store).await.expect("refresh").expect("now changed");
		assert_eq!(refreshed.by_sni.len(), 2);
		assert!(refreshed.by_sni.contains_key("b.example"));
	}

	#[tokio::test]
	async fn refresh_returns_some_when_cert_rotated() {
		let registry = registry_with(&[("a.example", make_stored_cert("a.example"))]).await;
		let populator = ManagedCertPopulator::new(Arc::clone(&registry), vec!["a.example".into()]);
		let initial = populator.initial_store().await.expect("initial_store");
		let initial_der = initial.by_sni.get("a.example").map(|e| leaf_der(e).to_vec()).unwrap();

		// Rotate: replace the cached cert with a freshly-generated
		// keypair (different DER). This mirrors what the renewal
		// scheduler does after a successful re-issuance.
		registry.cache_cert_for_test("a.example", Arc::new(make_stored_cert("a.example")));
		let refreshed = populator.refresh(&initial).await.expect("refresh").expect("rotated");
		let new_der = refreshed.by_sni.get("a.example").map(|e| leaf_der(e).to_vec()).unwrap();
		assert_ne!(initial_der, new_der, "rotated cert must surface as a new DER");
	}

	#[tokio::test]
	async fn declare_managed_runs_during_construction() {
		// The populator's contract with the registry: by the time it
		// returns, the registry knows which SNIs the listener wants.
		// declared_snis() is the readout side.
		let registry = registry_with(&[]).await;
		let _populator = ManagedCertPopulator::new(
			Arc::clone(&registry),
			vec!["a.example".into(), "b.example".into(), "a.example".into()], // dup
		);
		let declared = registry.declared_snis();
		assert_eq!(declared, vec!["a.example".to_owned(), "b.example".to_owned()]);
	}

	#[test]
	fn cert_stores_equivalent_handles_disjoint_keys() {
		use std::collections::HashMap;

		let entry_a = Arc::new(make_entry("a.example"));
		let entry_b = Arc::new(make_entry("b.example"));

		let mut a_map = HashMap::new();
		a_map.insert("a.example".to_owned(), Arc::clone(&entry_a));
		let mut b_map = HashMap::new();
		b_map.insert("b.example".to_owned(), Arc::clone(&entry_b));
		let store_a = CertStore { by_sni: a_map, default: None };
		let store_b = CertStore { by_sni: b_map, default: None };
		assert!(!cert_stores_equivalent(&store_a, &store_b));
	}

	fn make_entry(sni: &str) -> CertEntry {
		crate::crypto::install_default_provider();
		let stored = make_stored_cert(sni);
		stored_to_cert_entry(&stored).expect("cert entry")
	}
}
