//! `ManagedCertRegistry` — daemon-scoped owner of every piece of
//! ACME state per `spec/acme.md` § _Architecture_.
//!
//! Lifetime: constructed once at daemon boot from the operator's
//! [`AcmeStore`], lives until shutdown. Reload churn rebuilds
//! `ManagedCertPopulator` (FlowGraph-scoped, Stage 3) but **not**
//! the registry — accounts and issued certs survive reloads, so
//! ACME rate-limit ceilings aren't exposed to operator config-
//! cycling.
//!
//! Owns:
//!
//! - `accounts`: the live `instant-acme::Account` clients keyed by
//!   directory URL hash — Stage 5 wires construction.
//! - `pending`: in-flight HTTP-01 / DNS-01 challenge tokens.
//!   Consulted by [`crate::fetch::acme_challenge::AcmeChallengeFetch`]
//!   on every `/.well-known/acme-challenge/<token>` request and
//!   cleaned up when issuance completes (or fails).
//! - `certs`: in-memory cache of issued certs, keyed by SNI.
//! - `schedule`: renewal scheduler stub for Stage 3.
//! - `store`: the persistence trait object.
//!
//! `issue_http01` is the issuance entry point but lives in a
//! follow-up commit (Stage 5 in this PR's commit ordering); the
//! placeholder here panics if invoked, which is fine because the
//! daemon doesn't call it until `kick_off_boot_issuance` lands.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tracing::{instrument, warn};

use super::store::{AcmeAccount, AcmeStore, StoreError, StoredCert};

/// Lookup key for the pending-challenge table. Per
/// `spec/acme.md` § _HTTP-01 § Case 1_ the responder verifies
/// **both** the URL token tail and the `Host` header before
/// returning the key authorisation — otherwise a misrouted CA
/// validator query could leak our key authorisation to a different
/// virtual host.
pub type ChallengeKey = (String, String); // (host_lowercase, token)

/// One in-flight HTTP-01 / DNS-01 challenge, registered by
/// [`ManagedCertRegistry::register_http01`] and looked up by the
/// `:80` challenge fetch.
#[derive(Clone, Debug)]
pub struct PendingChallenge {
	pub key_authorization: String,
	/// Soft TTL (issuance budget). The registry doesn't actively
	/// expire entries — issuance code drops them on success or
	/// failure — but operators reading `get_certs` can compare
	/// `issued_at` to `now()` and detect stuck challenges.
	pub issued_at: std::time::SystemTime,
}

/// Daemon-scoped owner of every piece of ACME state. Cheap to
/// share via `Arc`; all interior state is `DashMap` /
/// `parking_lot::Mutex` for fine-grained concurrency.
///
/// Construct via [`Self::open`]; the constructor hydrates the in-
/// memory cert cache from the [`AcmeStore`] so the very first
/// listener handshake post-reload has a hot lookup path.
pub struct ManagedCertRegistry {
	/// Persistence backend. The trait object lets operators swap
	/// `FsAcmeStore` for an alternative impl (object store, secret
	/// manager) without touching the registry.
	store: Arc<dyn AcmeStore>,
	/// In-memory mirror of the on-disk cert state, keyed by SNI
	/// (lowercased). Read on every TLS handshake via the populator
	/// (Stage 3); written by issuance + renewal paths.
	certs: DashMap<String, Arc<StoredCert>>,
	/// Active challenge tokens. Keyed by `(Host, token)` per
	/// `spec/acme.md` § _HTTP-01_; entries are added at issuance
	/// start and removed on success/failure.
	pending: DashMap<ChallengeKey, PendingChallenge>,
	/// `instant-acme` account clients keyed by directory URL hash.
	/// Built lazily on first issuance for a given directory; reused
	/// across subsequent issuances against the same CA.
	///
	/// Stage 5 wires the construction logic in
	/// [`Self::issue_http01`]; Stage 4 only owns the storage. Read
	/// via [`Self::cached_account`] / written via
	/// [`Self::cache_account`].
	#[allow(dead_code, reason = "wired in the issue_http01 commit")]
	accounts: parking_lot::Mutex<BTreeMap<String, Arc<AcmeAccount>>>,
	/// SNIs the registry has been told to consider managed. Updated
	/// by [`Self::declare_managed`] on every reload that swaps the
	/// `FlowGraph`; the boot-time issuance hook walks this set.
	declared: DashMap<String, ()>,
	/// Renewal scheduler handle. Stage 1 leaves this an inert
	/// placeholder; Stage 3 fills in the periodic timer + ARI
	/// poller per `spec/acme.md` § _Renewal triggers_.
	#[allow(dead_code)]
	schedule: Arc<RenewalScheduler>,
}

/// Stage 3 will replace this stub with the real periodic timer +
/// ARI poller. Exists now so the registry's struct shape doesn't
/// need to churn when Stage 3 lands; downstream code can already
/// take an `Arc<RenewalScheduler>` parameter.
#[derive(Debug, Default)]
pub struct RenewalScheduler {
	#[allow(dead_code)]
	pub(super) tick_interval: Duration,
}

impl RenewalScheduler {
	#[must_use]
	pub fn new() -> Self {
		// 5-minute tick per `spec/acme.md` § _Renewal triggers_;
		// matches `08-tls.md`'s `refresh()` cadence.
		Self { tick_interval: Duration::from_mins(5) }
	}
}

impl ManagedCertRegistry {
	/// Open the registry over `store` and hydrate the in-memory
	/// cert cache. Called once at daemon boot.
	///
	/// # Errors
	/// Returns [`RegistryError::Store`] when `list_cert_snis` or any
	/// individual `load_cert` fails. Boot fails closed: the daemon
	/// can't sensibly run with a partially-hydrated cache because
	/// that would let some SNIs trigger redundant issuances on
	/// first request.
	#[instrument(skip(store))]
	pub async fn open(store: Arc<dyn AcmeStore>) -> Result<Arc<Self>, RegistryError> {
		let registry = Arc::new(Self {
			store,
			certs: DashMap::new(),
			pending: DashMap::new(),
			accounts: parking_lot::Mutex::new(BTreeMap::new()),
			declared: DashMap::new(),
			schedule: Arc::new(RenewalScheduler::new()),
		});
		registry.hydrate().await?;
		Ok(registry)
	}

	/// Boot-time hydration: walk the store and fill `certs`. Called
	/// from [`Self::open`].
	async fn hydrate(&self) -> Result<(), RegistryError> {
		let snis = self.store.list_cert_snis().await?;
		for sni in snis {
			match self.store.load_cert(&sni).await? {
				Some(cert) => {
					self.certs.insert(sni, Arc::new(cert));
				}
				None => {
					// `list_cert_snis` and `load_cert` are individually
					// atomic but the pair isn't — a sibling delete
					// between calls is benign (no cert to load).
					warn!(target: "vane::acme", sni, "cert listed but load returned None");
				}
			}
		}
		Ok(())
	}

	/// Look up a cert by SNI (lowercased). Returns the cached
	/// `Arc<StoredCert>` when one is available, `None` otherwise.
	/// Called by `ManagedCertPopulator` (Stage 3) on every refresh.
	#[must_use]
	pub fn cert_for(&self, sni: &str) -> Option<Arc<StoredCert>> {
		let key = sni.to_ascii_lowercase();
		self.certs.get(&key).map(|r| Arc::clone(&*r))
	}

	/// Register the SNIs the new `FlowGraph` wants managed and
	/// return the subset that lacks a cached cert (those need
	/// first-time issuance).
	///
	/// Called by Stage 7's boot-time issuance hook after each
	/// successful `FlowGraph::link`. Idempotent — re-registering
	/// the same SNI is a no-op.
	pub fn declare_managed(&self, snis: &[String]) -> Vec<String> {
		let mut needs_issue = Vec::new();
		for sni in snis {
			let key = sni.to_ascii_lowercase();
			self.declared.insert(key.clone(), ());
			if !self.certs.contains_key(&key) {
				needs_issue.push(key);
			}
		}
		needs_issue
	}

	/// Snapshot of every SNI currently declared managed. Stable
	/// order (sorted) so callers diffing across reloads observe a
	/// deterministic sequence.
	#[must_use]
	pub fn declared_snis(&self) -> Vec<String> {
		let mut out: Vec<String> = self.declared.iter().map(|e| e.key().clone()).collect();
		out.sort();
		out
	}

	/// Register an in-flight HTTP-01 challenge. Called by
	/// `issue_http01` once it has constructed the key authorisation
	/// for a particular `(host, token)` pair; the `:80` fetch reads
	/// this table to satisfy the CA validator.
	pub fn register_http01(&self, host: &str, token: String, key_authorization: String) {
		self.pending.insert(
			(host.to_ascii_lowercase(), token),
			PendingChallenge { key_authorization, issued_at: std::time::SystemTime::now() },
		);
	}

	/// Read-side counterpart of [`Self::register_http01`]. Called by
	/// `AcmeChallengeFetch::fetch` on every
	/// `/.well-known/acme-challenge/<token>` request.
	#[must_use]
	pub fn lookup_http01(&self, host: &str, token: &str) -> Option<String> {
		let key = (host.to_ascii_lowercase(), token.to_owned());
		self.pending.get(&key).map(|e| e.key_authorization.clone())
	}

	/// Remove an in-flight challenge. Called on success and on
	/// failure (RAII guard in `issue_http01` ensures cleanup even
	/// on `?` short-circuit).
	pub fn unregister_http01(&self, host: &str, token: &str) {
		let key = (host.to_ascii_lowercase(), token.to_owned());
		self.pending.remove(&key);
	}

	/// Update the in-memory cache. Used by `save_cert_in_place` and
	/// `issue_http01` after they persist a fresh cert.
	#[allow(dead_code, reason = "wired in the issue_http01 commit")]
	pub(super) fn cache_cert(&self, sni: &str, cert: Arc<StoredCert>) {
		self.certs.insert(sni.to_ascii_lowercase(), cert);
	}

	/// Cached account for `directory_url`, if loaded. Used by
	/// `issue_http01` to short-circuit account creation when the
	/// directory has been used before in this daemon lifetime.
	#[must_use]
	#[allow(dead_code, reason = "wired in the issue_http01 commit")]
	pub(super) fn cached_account(&self, directory_url_hash: &str) -> Option<Arc<AcmeAccount>> {
		self.accounts.lock().get(directory_url_hash).cloned()
	}

	/// Cache an account in memory after a successful load or
	/// create. The store-side persistence is the caller's job —
	/// this only touches in-memory state.
	#[allow(dead_code, reason = "wired in the issue_http01 commit")]
	pub(super) fn cache_account(&self, directory_url_hash: String, account: Arc<AcmeAccount>) {
		self.accounts.lock().insert(directory_url_hash, account);
	}

	/// Borrow the underlying store. Stage 5 issuance code reads
	/// account material through the same trait object the registry
	/// was constructed with.
	#[must_use]
	#[allow(dead_code, reason = "wired in the issue_http01 commit")]
	pub(super) fn store(&self) -> &dyn AcmeStore {
		&*self.store
	}

	/// Issue a cert for `sni` via the HTTP-01 challenge. The
	/// implementation lives in the next commit; this stub keeps the
	/// public surface stable for Stage 7's boot-time hook.
	///
	/// # Errors
	/// All variants of [`RegistryError`] are reachable from the
	/// real impl; the stub only ever returns
	/// `RegistryError::Internal("issue_http01 not yet implemented")`.
	#[allow(clippy::unused_async, reason = "real impl is async; stub keeps signature stable")]
	pub async fn issue_http01(
		&self,
		sni: &str,
		_directory_url: &str,
		_contact: &[String],
	) -> Result<Arc<StoredCert>, RegistryError> {
		// Use `sni` so the unimplemented log line is greppable per-SNI
		// when grepping daemon logs during Stage 5 development.
		Err(RegistryError::Internal(format!("issue_http01 not yet implemented (sni={sni})")))
	}
}

/// Errors surfaced by [`ManagedCertRegistry`]. Categorised so the
/// Stage 3 backoff scheduler can branch on `RateLimited` without
/// string-matching the CA's response body.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
	#[error("storage: {0}")]
	Store(#[from] StoreError),
	#[error("acme protocol: {0}")]
	Acme(String),
	#[error("http-01 validation timeout for {0}")]
	Http01Timeout(String),
	#[error("rate limited by ACME server")]
	RateLimited {
		/// CA-suggested retry-after, when the response carried one
		/// (instant-acme surfaces it from the `Retry-After` header).
		retry_after: Option<Duration>,
	},
	#[error("internal: {0}")]
	Internal(String),
}

#[cfg(test)]
mod tests {
	use std::time::SystemTime;

	use async_trait::async_trait;

	use super::*;
	use crate::acme::store::{AcmeAccount, AcmeStore, LockGuard, StoreError, StoredCert};

	/// In-memory mock store for unit tests. `Arc<MockStore>` works
	/// as `Arc<dyn AcmeStore>` so registry construction matches
	/// production wiring.
	#[derive(Default)]
	struct MockStore {
		accounts: parking_lot::Mutex<BTreeMap<String, AcmeAccount>>,
		certs: parking_lot::Mutex<BTreeMap<String, StoredCert>>,
	}

	#[derive(Debug)]
	struct MockGuard;
	impl LockGuard for MockGuard {}

	#[async_trait]
	impl AcmeStore for MockStore {
		async fn load_account(&self, directory_url: &str) -> Result<Option<AcmeAccount>, StoreError> {
			Ok(self.accounts.lock().get(directory_url).cloned())
		}
		async fn save_account(
			&self,
			directory_url: &str,
			account: &AcmeAccount,
		) -> Result<(), StoreError> {
			self.accounts.lock().insert(directory_url.to_owned(), account.clone());
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

	fn fixture_cert() -> StoredCert {
		StoredCert {
			leaf_pem: "leaf".into(),
			chain_pem: String::new(),
			key_pem: "key".into(),
			not_after: SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000),
			ari_replacement_id: None,
			last_renew_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
		}
	}

	#[tokio::test]
	async fn open_hydrates_cached_certs() {
		let store = Arc::new(MockStore::default());
		store.save_cert("api.example.com", &fixture_cert()).await.unwrap();
		store.save_cert("admin.example.com", &fixture_cert()).await.unwrap();
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		assert!(registry.cert_for("api.example.com").is_some());
		assert!(registry.cert_for("admin.example.com").is_some());
		assert!(registry.cert_for("unknown.example.com").is_none());
	}

	#[tokio::test]
	async fn cert_for_lowercases_sni() {
		let store = Arc::new(MockStore::default());
		store.save_cert("api.example.com", &fixture_cert()).await.unwrap();
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		assert!(registry.cert_for("API.example.COM").is_some());
	}

	#[tokio::test]
	async fn declare_managed_returns_only_uncached() {
		let store = Arc::new(MockStore::default());
		store.save_cert("api.example.com", &fixture_cert()).await.unwrap();
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		let needs = registry.declare_managed(&[
			"api.example.com".into(),
			"admin.example.com".into(),
			"www.example.com".into(),
		]);
		assert_eq!(needs, vec!["admin.example.com", "www.example.com"]);
	}

	#[tokio::test]
	async fn declare_managed_is_idempotent() {
		let store = Arc::new(MockStore::default());
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		let _ = registry.declare_managed(&["x.example.com".into()]);
		let _ = registry.declare_managed(&["x.example.com".into()]);
		assert_eq!(registry.declared_snis(), vec!["x.example.com"]);
	}

	#[tokio::test]
	async fn http01_register_lookup_unregister_cycle() {
		let store = Arc::new(MockStore::default());
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		registry.register_http01("api.example.com", "tok-XYZ".into(), "ka-ABC".into());
		assert_eq!(registry.lookup_http01("api.example.com", "tok-XYZ").as_deref(), Some("ka-ABC"),);
		registry.unregister_http01("api.example.com", "tok-XYZ");
		assert!(registry.lookup_http01("api.example.com", "tok-XYZ").is_none());
	}

	#[tokio::test]
	async fn http01_lookup_lowercases_host() {
		let store = Arc::new(MockStore::default());
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		registry.register_http01("api.example.com", "tok".into(), "key".into());
		assert!(registry.lookup_http01("API.EXAMPLE.COM", "tok").is_some());
	}

	#[tokio::test]
	async fn http01_lookup_distinguishes_hosts() {
		let store = Arc::new(MockStore::default());
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		registry.register_http01("api.example.com", "tok".into(), "key-api".into());
		assert!(registry.lookup_http01("admin.example.com", "tok").is_none());
		assert_eq!(registry.lookup_http01("api.example.com", "tok").as_deref(), Some("key-api"),);
	}

	#[tokio::test]
	async fn issue_http01_stub_returns_internal_error() {
		let store = Arc::new(MockStore::default());
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		match registry
			.issue_http01("api.example.com", "https://acme/dir", &["mailto:ops@example.com".into()])
			.await
		{
			Err(RegistryError::Internal(_)) => {}
			other => panic!("expected stub Internal, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn cache_cert_overwrites_prior_entry() {
		let store = Arc::new(MockStore::default());
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		let mut cert = fixture_cert();
		cert.leaf_pem = "v1".into();
		registry.cache_cert("api.example.com", Arc::new(cert.clone()));
		cert.leaf_pem = "v2".into();
		registry.cache_cert("api.example.com", Arc::new(cert));
		assert_eq!(registry.cert_for("api.example.com").unwrap().leaf_pem, "v2");
	}
}
