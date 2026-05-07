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
//! - `live_accounts`: live `instant-acme::Account` HTTP clients
//!   keyed by `directory_url`, lazily built on first issuance.
//! - `pending`: in-flight HTTP-01 / DNS-01 challenge tokens, keyed
//!   by `(host, token)`. Consulted by `AcmeChallengeFetch` on every
//!   `/.well-known/acme-challenge/<token>` request and cleaned up
//!   when issuance completes (RAII guard) or fails.
//! - `certs`: in-memory cache of issued certs, keyed by SNI.
//! - `schedule`: renewal scheduler stub for Stage 3.
//! - `store`: the persistence trait object (typically `FsAcmeStore`).
//!
//! Issuance entry points: [`ManagedCertRegistry::issue_http01`] for
//! production (default trust roots) and
//! [`ManagedCertRegistry::issue_http01_with_root`] for test harnesses
//! that need a custom CA root (Pebble).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use sha2::Digest;
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
	/// Live `instant-acme` account clients keyed by `directory_url`.
	/// Built lazily by [`Self::account_for`] on first issuance for
	/// a given directory; reused across subsequent issuances against
	/// the same CA. The persisted account material lives in
	/// [`Self::store`]; this map only caches the live HTTP client.
	live_accounts: parking_lot::Mutex<BTreeMap<String, Arc<instant_acme::Account>>>,
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
			live_accounts: parking_lot::Mutex::new(BTreeMap::new()),
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

	/// Update the in-memory cache. Called by `issue_http01` after
	/// it persists a fresh cert via the store.
	pub(super) fn cache_cert(&self, sni: &str, cert: Arc<StoredCert>) {
		self.certs.insert(sni.to_ascii_lowercase(), cert);
	}

	/// Acquire (load-or-create) the live `instant-acme::Account`
	/// for `directory_url`, persisting fresh credentials to the
	/// store and caching the live client in [`Self::live_accounts`].
	///
	/// Locking: holds an `account/<hash>` advisory lock for the
	/// load-or-create span so two boot-time issuance tasks racing
	/// the same directory URL don't both ask the CA to register.
	///
	/// Atomicity: a fresh `Account::create` returns
	/// `(Account, AccountCredentials)`. We persist the credentials
	/// **before** returning the live account so a pkill during
	/// issuance doesn't leave us with an unrecoverable
	/// CA-registered account whose key we've lost. The store's
	/// `save_account` is itself atomic (tmp + rename + fsync).
	async fn account_for(
		&self,
		directory_url: &str,
		contact: &[String],
		extra_root_ca_pem: Option<&std::path::Path>,
	) -> Result<Arc<instant_acme::Account>, RegistryError> {
		// Fast path: already live for this directory.
		if let Some(live) = self.live_accounts.lock().get(directory_url).cloned() {
			return Ok(live);
		}

		// Slow path: serialised across tasks + processes by the
		// store's advisory lock keyed on the directory URL hash.
		let scope = format!("account/{}", directory_url_scope(directory_url));
		let _guard = self.store.lock(&scope).await?;

		// Re-check after acquiring the lock — another task may have
		// raced ahead and populated the cache while we waited.
		if let Some(live) = self.live_accounts.lock().get(directory_url).cloned() {
			return Ok(live);
		}

		if let Some(stored) = self.store.load_account(directory_url).await? {
			let creds: instant_acme::AccountCredentials = serde_json::from_value(stored.key_jwk)
				.map_err(|e| RegistryError::Acme(format!("decode account credentials: {e}")))?;
			let builder = build_account_builder(extra_root_ca_pem)?;
			let live = builder.from_credentials(creds).await.map_err(map_acme_error)?;
			let live = Arc::new(live);
			self.live_accounts.lock().insert(directory_url.to_owned(), Arc::clone(&live));
			return Ok(live);
		}

		// Fresh registration. Convert &[String] → &[&str] for NewAccount.
		let contact_refs: Vec<&str> = contact.iter().map(String::as_str).collect();
		let new_account = instant_acme::NewAccount {
			contact: &contact_refs,
			terms_of_service_agreed: true,
			only_return_existing: false,
		};
		let builder = build_account_builder(extra_root_ca_pem)?;
		let (live, creds) =
			builder.create(&new_account, directory_url.to_owned(), None).await.map_err(map_acme_error)?;

		// Persist before returning. Failure here means we have a
		// CA-side account we can't recover — surface as Store error
		// so the boot hook logs at ERROR and operators see it.
		let key_jwk = serde_json::to_value(&creds)
			.map_err(|e| RegistryError::Acme(format!("encode account credentials: {e}")))?;
		let acme_account = AcmeAccount {
			directory_url: directory_url.to_owned(),
			key_jwk,
			kid: live.id().to_owned(),
			contacts: contact.to_vec(),
			agreed_tos_at: std::time::SystemTime::now(),
		};
		self.store.save_account(directory_url, &acme_account).await?;

		let live = Arc::new(live);
		self.live_accounts.lock().insert(directory_url.to_owned(), Arc::clone(&live));
		Ok(live)
	}

	/// Issue a cert for `sni` via the HTTP-01 challenge.
	///
	/// Walks the RFC 8555 issuance sequence end-to-end:
	///
	/// 1. Acquire the live ACME account for `directory_url`.
	/// 2. Place a new order for the SAN list (Stage 1: `[sni]`).
	/// 3. Stream-walk authorisations, register each HTTP-01 token in
	///    the registry's `pending` table, and signal the challenge
	///    ready to the CA.
	/// 4. Poll the order until `Ready`.
	/// 5. Generate an ECDSA P-256 keypair + CSR via `rcgen`.
	/// 6. Finalize the order with the CSR.
	/// 7. Poll until the cert chain PEM is downloadable.
	/// 8. Parse `not_after`, persist the [`StoredCert`] to the
	///    store, populate the in-memory cache.
	///
	/// Cleanup: a RAII [`ChallengeCleanup`] guard removes pending
	/// challenges from the registry's `pending` table on every exit
	/// path — including `?` short-circuits — so a failed issuance
	/// doesn't leak entries.
	///
	/// # Errors
	///
	/// - [`RegistryError::Store`]: filesystem failure persisting
	///   credentials or the issued cert.
	/// - [`RegistryError::Acme`]: any `instant-acme` failure
	///   (network, ACME protocol, JSON parse).
	/// - [`RegistryError::RateLimited`]: CA returned
	///   `urn:ietf:params:acme:error:rateLimited`.
	/// - [`RegistryError::Http01Timeout`]: the order didn't reach
	///   `Ready` within the issuance budget.
	#[instrument(skip(self), fields(directory_url))]
	pub async fn issue_http01(
		&self,
		sni: &str,
		directory_url: &str,
		contact: &[String],
	) -> Result<Arc<StoredCert>, RegistryError> {
		self.issue_http01_inner(sni, directory_url, contact, None).await
	}

	/// Variant of [`Self::issue_http01`] that threads a custom root
	/// CA into the `instant-acme` HTTP client. Used by integration
	/// tests against Pebble (which uses a self-signed root).
	///
	/// # Errors
	/// Identical to [`Self::issue_http01`].
	#[instrument(skip(self, extra_root_ca_pem), fields(directory_url))]
	pub async fn issue_http01_with_root(
		&self,
		sni: &str,
		directory_url: &str,
		contact: &[String],
		extra_root_ca_pem: &std::path::Path,
	) -> Result<Arc<StoredCert>, RegistryError> {
		self.issue_http01_inner(sni, directory_url, contact, Some(extra_root_ca_pem)).await
	}

	/// Issue a cert for `sni` via the DNS-01 challenge.
	///
	/// Mirrors the [`Self::issue_http01`] flow but routes the
	/// challenge through the operator-provided
	/// [`super::DnsProvider`]: walk the order's authorisations,
	/// `set_txt(_acme-challenge.<sni>, dns_value)` per identifier,
	/// `wait_propagated` against the provider's resolver pool,
	/// signal challenge ready, finalize, download the cert,
	/// `delete_txt` to clean up.
	///
	/// `sni` may be a wildcard (`*.example.com`); the ACME server
	/// returns a non-wildcard identifier in the authz, so the TXT
	/// record always lands at `_acme-challenge.<base>` with the
	/// `*.` prefix stripped.
	///
	/// Cleanup: on every exit path (success, `?`, panic) the
	/// [`DnsCleanupGuard`] drops and best-effort `delete_txt`s
	/// every TXT record this issuance set. On success the guard's
	/// inline cleanup runs synchronously so the operator's DNS
	/// state is in a known-clean state when the function returns.
	///
	/// # Errors
	/// Same shape as [`Self::issue_http01`] plus DNS provider
	/// failures surfaced as [`RegistryError::Acme`].
	#[instrument(skip(self, dns), fields(directory_url))]
	pub async fn issue_dns01(
		&self,
		sni: &str,
		directory_url: &str,
		contact: &[String],
		dns: Arc<dyn super::DnsProvider>,
	) -> Result<Arc<StoredCert>, RegistryError> {
		self.issue_dns01_inner(sni, directory_url, contact, None, dns).await
	}

	/// Test-harness variant of [`Self::issue_dns01`] that threads a
	/// custom root CA into the `instant-acme` HTTP client.
	///
	/// # Errors
	/// Identical to [`Self::issue_dns01`].
	#[instrument(skip(self, extra_root_ca_pem, dns), fields(directory_url))]
	pub async fn issue_dns01_with_root(
		&self,
		sni: &str,
		directory_url: &str,
		contact: &[String],
		extra_root_ca_pem: &std::path::Path,
		dns: Arc<dyn super::DnsProvider>,
	) -> Result<Arc<StoredCert>, RegistryError> {
		self.issue_dns01_inner(sni, directory_url, contact, Some(extra_root_ca_pem), dns).await
	}

	async fn issue_dns01_inner(
		&self,
		sni: &str,
		directory_url: &str,
		contact: &[String],
		extra_root_ca_pem: Option<&std::path::Path>,
		dns: Arc<dyn super::DnsProvider>,
	) -> Result<Arc<StoredCert>, RegistryError> {
		let cert_scope = format!("cert/{sni}");
		let _cert_lock = self.store.lock(&cert_scope).await?;

		if let Some(existing) = self.cert_for(sni) {
			return Ok(existing);
		}

		let account = self.account_for(directory_url, contact, extra_root_ca_pem).await?;
		let identifiers = vec![instant_acme::Identifier::Dns(sni.to_owned())];
		let new_order = instant_acme::NewOrder::new(&identifiers);
		let mut order = account.new_order(&new_order).await.map_err(map_acme_error)?;

		let cleanup = DnsCleanupGuard::new(Arc::clone(&dns));
		register_dns01_challenges(&*dns, &mut order, &cleanup).await?;

		let retry = instant_acme::RetryPolicy::default()
			.timeout(Duration::from_mins(1))
			.initial_delay(Duration::from_millis(250))
			.backoff(2.0);
		match order.poll_ready(&retry).await.map_err(map_acme_error)? {
			instant_acme::OrderStatus::Ready => {}
			other => {
				return Err(RegistryError::Http01Timeout(format!(
					"order for {sni:?} stalled at {other:?} (expected Ready)"
				)));
			}
		}

		let csr_sni = sni.to_owned();
		let (key_pem, csr_der) = generate_ecdsa_p256_csr(&csr_sni)?;
		order.finalize_csr(&csr_der).await.map_err(map_acme_error)?;
		let chain_pem = order.poll_certificate(&retry).await.map_err(map_acme_error)?;

		let (leaf_pem, intermediates_pem) = split_leaf_chain(&chain_pem);
		let not_after = parse_not_after_pem(&leaf_pem)?;
		let now = std::time::SystemTime::now();
		let stored = StoredCert {
			leaf_pem,
			chain_pem: intermediates_pem,
			key_pem,
			not_after,
			ari_replacement_id: None,
			last_renew_at: now,
		};
		self.store.save_cert(sni, &stored).await?;
		let arc = Arc::new(stored);
		self.cache_cert(sni, Arc::clone(&arc));

		// Synchronous cleanup on success so the operator's DNS state
		// is known-clean by the time the function returns. The guard
		// is now disarmed and drops as a no-op.
		cleanup.cleanup_now().await;
		Ok(arc)
	}

	async fn issue_http01_inner(
		&self,
		sni: &str,
		directory_url: &str,
		contact: &[String],
		extra_root_ca_pem: Option<&std::path::Path>,
	) -> Result<Arc<StoredCert>, RegistryError> {
		let cert_scope = format!("cert/{sni}");
		let _cert_lock = self.store.lock(&cert_scope).await?;

		// If the cache already has a cert (race vs another task on
		// the same SNI), short-circuit. Stage 3's scheduler handles
		// renewal triggers separately.
		if let Some(existing) = self.cert_for(sni) {
			return Ok(existing);
		}

		let account = self.account_for(directory_url, contact, extra_root_ca_pem).await?;
		let identifiers = vec![instant_acme::Identifier::Dns(sni.to_owned())];
		let new_order = instant_acme::NewOrder::new(&identifiers);
		let mut order = account.new_order(&new_order).await.map_err(map_acme_error)?;

		// Walk authorizations + register http-01 challenges. The
		// cleanup guard tracks every (host, token) so panics, ?
		// short-circuits, and Ok returns all unregister cleanly.
		let mut cleanup = ChallengeCleanup::new(self);
		register_http01_challenges(self, &mut order, &mut cleanup).await?;

		// Poll the order through Pending → Ready. instant-acme's
		// RetryPolicy default is 5s; managed-CA HTTP-01 validation
		// often takes 10–30s, so widen the timeout to 60s with a
		// 250ms initial delay (matches the default cadence).
		let retry = instant_acme::RetryPolicy::default()
			.timeout(Duration::from_mins(1))
			.initial_delay(Duration::from_millis(250))
			.backoff(2.0);
		match order.poll_ready(&retry).await.map_err(map_acme_error)? {
			instant_acme::OrderStatus::Ready => {}
			other => {
				return Err(RegistryError::Http01Timeout(format!(
					"order for {sni:?} stalled at {other:?} (expected Ready)"
				)));
			}
		}

		// Generate keypair + CSR. ECDSA P-256 is the Stage 1
		// hard-coded choice; `tls.managed.key_type` flows in here
		// once Stage 7 wires the boot hook.
		let (key_pem, csr_der) = generate_ecdsa_p256_csr(sni)?;
		order.finalize_csr(&csr_der).await.map_err(map_acme_error)?;
		let chain_pem = order.poll_certificate(&retry).await.map_err(map_acme_error)?;

		let (leaf_pem, intermediates_pem) = split_leaf_chain(&chain_pem);
		let not_after = parse_not_after_pem(&leaf_pem)?;
		let now = std::time::SystemTime::now();
		let stored = StoredCert {
			leaf_pem,
			chain_pem: intermediates_pem,
			key_pem,
			not_after,
			ari_replacement_id: None,
			last_renew_at: now,
		};
		self.store.save_cert(sni, &stored).await?;
		let arc = Arc::new(stored);
		self.cache_cert(sni, Arc::clone(&arc));

		// Cleanup runs on guard drop — explicit to make the
		// intent visible at the success-path bottom.
		drop(cleanup);
		Ok(arc)
	}
}

/// RAII tracker for HTTP-01 challenge tokens registered during a
/// single [`ManagedCertRegistry::issue_http01`] call. On drop —
/// whether via normal return, `?` short-circuit, or panic — every
/// tracked entry is removed from the registry's `pending` table.
struct ChallengeCleanup<'a> {
	registry: &'a ManagedCertRegistry,
	keys: Vec<(String, String)>,
}

impl<'a> ChallengeCleanup<'a> {
	fn new(registry: &'a ManagedCertRegistry) -> Self {
		Self { registry, keys: Vec::new() }
	}

	fn track(&mut self, host: String, token: String) {
		self.keys.push((host, token));
	}
}

impl Drop for ChallengeCleanup<'_> {
	fn drop(&mut self) {
		for (host, token) in self.keys.drain(..) {
			self.registry.unregister_http01(&host, &token);
		}
	}
}

/// Walk the order's authorisations stream, register every HTTP-01
/// challenge token in the registry's pending table, signal each
/// ready to the CA. Returns when every authorisation has been
/// signalled; failures short-circuit with cleanup running through
/// the [`ChallengeCleanup`] guard the caller passes in.
async fn register_http01_challenges(
	registry: &ManagedCertRegistry,
	order: &mut instant_acme::Order,
	cleanup: &mut ChallengeCleanup<'_>,
) -> Result<(), RegistryError> {
	let mut auth_stream = order.authorizations();
	while let Some(item) = auth_stream.next().await {
		let mut authz = item.map_err(map_acme_error)?;
		// Read the http-01 token directly off the AuthorizationState
		// (AuthorizationHandle Derefs to it). Cloning the token frees
		// the borrow before we call `.challenge()` below, which
		// needs `&mut self` on the handle.
		let token = authz
			.challenges
			.iter()
			.find(|c| c.r#type == instant_acme::ChallengeType::Http01)
			.map(|c| c.token.clone())
			.ok_or_else(|| RegistryError::Acme("no http-01 challenge offered".into()))?;
		let host = match &authz.identifier().identifier {
			instant_acme::Identifier::Dns(s) => s.clone(),
			other => {
				return Err(RegistryError::Acme(format!(
					"unexpected identifier kind for http-01: {other:?}"
				)));
			}
		};
		let mut handle = authz
			.challenge(instant_acme::ChallengeType::Http01)
			.ok_or_else(|| RegistryError::Acme("no http-01 challenge handle".into()))?;
		let key_auth = handle.key_authorization().as_str().to_owned();
		registry.register_http01(&host, token.clone(), key_auth);
		cleanup.track(host, token);
		handle.set_ready().await.map_err(map_acme_error)?;
	}
	Ok(())
}

/// DNS-01 counterpart of [`register_http01_challenges`]. Walks
/// the order's authorisations, computes the
/// `base64url(sha256(key_authorization))` value RFC 8555 §8.4
/// expects in the TXT record, calls `set_txt` + `wait_propagated`
/// per identifier, then signals each challenge ready.
///
/// Wildcard SANs: ACME servers strip the `*.` prefix before
/// emitting the authz identifier, so the TXT name we set is
/// always `_acme-challenge.<base-domain>` — no wildcard handling
/// needed here.
async fn register_dns01_challenges(
	dns: &dyn super::DnsProvider,
	order: &mut instant_acme::Order,
	cleanup: &DnsCleanupGuard,
) -> Result<(), RegistryError> {
	// 120s aligns with `spec/acme.md` § _wait_propagated semantics_:
	// public DNS typically converges sub-minute even for fresh
	// records; doubling that gives headroom for stragglers without
	// burning operator patience on a stuck propagation.
	const PROPAGATION_TIMEOUT: Duration = Duration::from_mins(2);

	let mut auth_stream = order.authorizations();
	while let Some(item) = auth_stream.next().await {
		let mut authz = item.map_err(map_acme_error)?;
		let identifier = match &authz.identifier().identifier {
			instant_acme::Identifier::Dns(s) => s.clone(),
			other => {
				return Err(RegistryError::Acme(format!(
					"unexpected identifier kind for dns-01: {other:?}"
				)));
			}
		};
		let mut handle = authz
			.challenge(instant_acme::ChallengeType::Dns01)
			.ok_or_else(|| RegistryError::Acme("no dns-01 challenge offered".into()))?;
		let txt_value = handle.key_authorization().dns_value();
		let txt_name = dns_challenge_name(&identifier);
		dns.set_txt(&txt_name, &txt_value).await.map_err(|e| map_dns_error(&e))?;
		cleanup.track(txt_name.clone());
		dns
			.wait_propagated(&txt_name, &txt_value, PROPAGATION_TIMEOUT)
			.await
			.map_err(|e| map_dns_error(&e))?;
		handle.set_ready().await.map_err(map_acme_error)?;
	}
	Ok(())
}

/// Build the TXT record name the ACME server queries for a DNS-01
/// challenge. RFC 8555 §8.4: `_acme-challenge.<domain>`. Wildcard
/// authzs come through with the `*.` prefix already stripped, so
/// no special-case here — but we still defensively strip in case a
/// future ACME server breaks the convention.
fn dns_challenge_name(identifier: &str) -> String {
	let base = identifier.strip_prefix("*.").unwrap_or(identifier);
	format!("_acme-challenge.{base}")
}

/// RAII tracker for TXT records the DNS-01 issuance flow created.
/// `track` adds a name; `cleanup_now` drains the list and runs
/// `delete_txt` synchronously (success path); `Drop` falls back to
/// best-effort detached cleanup on `?` short-circuits / panics.
struct DnsCleanupGuard {
	dns: Arc<dyn super::DnsProvider>,
	names: parking_lot::Mutex<Vec<String>>,
}

impl DnsCleanupGuard {
	fn new(dns: Arc<dyn super::DnsProvider>) -> Self {
		Self { dns, names: parking_lot::Mutex::new(Vec::new()) }
	}

	fn track(&self, name: String) {
		self.names.lock().push(name);
	}

	/// Synchronous cleanup invoked at the success path's tail. The
	/// guard's `Drop` fires on a now-empty list and is a no-op.
	async fn cleanup_now(&self) {
		let names = std::mem::take(&mut *self.names.lock());
		for name in names {
			let _ = self.dns.delete_txt(&name).await;
		}
	}
}

impl Drop for DnsCleanupGuard {
	fn drop(&mut self) {
		let names = std::mem::take(&mut *self.names.lock());
		if names.is_empty() {
			return;
		}
		let dns = Arc::clone(&self.dns);
		// Best-effort detached cleanup. If the runtime is shutting
		// down (e.g. SIGTERM mid-issuance), the spawned task may
		// not get scheduled — but the alternative is blocking the
		// drop on async I/O, which Rust doesn't allow. Operators
		// can run a manual `delete_txt` cleanup if a daemon crash
		// leaves stale records.
		if let Ok(handle) = tokio::runtime::Handle::try_current() {
			handle.spawn(async move {
				for name in names {
					let _ = dns.delete_txt(&name).await;
				}
			});
		}
	}
}

/// Translate a [`super::DnsProviderError`] into a
/// [`RegistryError`]. Auth and zone-not-found surface as
/// `Acme(...)` because they're operator-config issues that block
/// issuance; rate-limit-style errors don't have a DNS analogue
/// (the public DNS API limits we care about — Cloudflare's
/// per-zone create limit — manifest as 4xx with a body that's
/// implementation-defined, so we surface them generically too).
fn map_dns_error(err: &super::DnsProviderError) -> RegistryError {
	RegistryError::Acme(err.to_string())
}

/// Generate an ECDSA P-256 keypair and a CSR for `sni`. Returns
/// the PKCS#8 PEM private key and the DER-encoded CSR.
fn generate_ecdsa_p256_csr(sni: &str) -> Result<(String, Vec<u8>), RegistryError> {
	let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
		.map_err(|e| RegistryError::Acme(format!("rcgen keypair: {e}")))?;
	let params = rcgen::CertificateParams::new(vec![sni.to_owned()])
		.map_err(|e| RegistryError::Acme(format!("rcgen params: {e}")))?;
	let csr = params
		.serialize_request(&key_pair)
		.map_err(|e| RegistryError::Acme(format!("rcgen csr: {e}")))?;
	let key_pem = key_pair.serialize_pem();
	let csr_der: Vec<u8> = csr.der().to_vec();
	Ok((key_pem, csr_der))
}

/// Split a `leaf+intermediate` PEM blob into the leaf and the rest
/// using the second `BEGIN CERTIFICATE` boundary as the cut point.
fn split_leaf_chain(pem: &str) -> (String, String) {
	const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
	let mut iter = pem.match_indices(BEGIN);
	let _first = iter.next();
	match iter.next() {
		Some((idx, _)) => (pem[..idx].to_owned(), pem[idx..].to_owned()),
		None => (pem.to_owned(), String::new()),
	}
}

/// Extract the leaf's `notAfter` from a PEM blob via `x509-parser`.
fn parse_not_after_pem(leaf_pem: &str) -> Result<std::time::SystemTime, RegistryError> {
	use x509_parser::prelude::FromDer;
	let der = rustls_pemfile::certs(&mut leaf_pem.as_bytes())
		.next()
		.ok_or_else(|| RegistryError::Acme("CA returned no certificate PEM".into()))?
		.map_err(|e| RegistryError::Acme(format!("PEM parse: {e}")))?;
	let (_, cert) = x509_parser::prelude::X509Certificate::from_der(der.as_ref())
		.map_err(|e| RegistryError::Acme(format!("x509 parse: {e}")))?;
	let secs = cert.validity().not_after.timestamp();
	let secs: u64 = u64::try_from(secs)
		.map_err(|_| RegistryError::Acme(format!("notAfter has negative epoch {secs}")))?;
	Ok(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

/// Translate `instant_acme::Error` into the registry's typed error
/// enum. Surfaces ACME rate-limit responses as a typed
/// [`RegistryError::RateLimited`] so the Stage 3 backoff scheduler
/// can branch on it without string-matching. Other errors carry
/// the full chained-cause render so transient connect / TLS
/// failures aren't reduced to "client error".
fn map_acme_error(err: instant_acme::Error) -> RegistryError {
	match err {
		instant_acme::Error::Api(problem)
			if problem.r#type.as_deref() == Some("urn:ietf:params:acme:error:rateLimited") =>
		{
			RegistryError::RateLimited { retry_after: None }
		}
		other => RegistryError::Acme(format_chained(&other)),
	}
}

fn format_chained(err: &(dyn std::error::Error + 'static)) -> String {
	use std::fmt::Write as _;
	let mut out = err.to_string();
	let mut src = err.source();
	while let Some(e) = src {
		let _ = write!(out, ": {e}");
		src = e.source();
	}
	out
}

/// Build an `instant_acme::AccountBuilder`. `extra_root_ca_pem` is
/// a path to a PEM file containing a trusted root for the CA's
/// HTTPS endpoint — used by Pebble integration tests.
fn build_account_builder(
	extra_root_ca_pem: Option<&std::path::Path>,
) -> Result<instant_acme::AccountBuilder, RegistryError> {
	match extra_root_ca_pem {
		Some(path) => instant_acme::Account::builder_with_root(path)
			.map_err(|e| RegistryError::Acme(format!("instant-acme builder_with_root: {e}"))),
		None => instant_acme::Account::builder()
			.map_err(|e| RegistryError::Acme(format!("instant-acme builder: {e}"))),
	}
}

/// `sha256(directory_url)[..16]` — matches the [`super::FsAcmeStore`]
/// account directory naming so the [`AcmeStore::lock`] scope
/// translates to the right `.lock` file path.
fn directory_url_scope(directory_url: &str) -> String {
	use std::fmt::Write as _;
	let digest = sha2::Sha256::digest(directory_url.as_bytes());
	let mut hex = String::with_capacity(64);
	for b in &digest {
		let _ = write!(hex, "{b:02x}");
	}
	hex.chars().take(16).collect()
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
	async fn issue_http01_short_circuits_when_cert_already_cached() {
		// When a cert is already in the registry's cache (e.g. from
		// a prior boot's hydration), issue_http01 returns the
		// cached value without touching the network. This is the
		// only network-free assertion we can make about the public
		// surface; full issuance flows live in the Pebble e2e tests.
		let store = Arc::new(MockStore::default());
		store.save_cert("api.example.com", &fixture_cert()).await.unwrap();
		let registry = ManagedCertRegistry::open(store as Arc<dyn AcmeStore>).await.unwrap();
		let got = registry
			.issue_http01(
				"api.example.com",
				"https://acme.invalid/dir",
				&["mailto:ops@example.com".into()],
			)
			.await
			.expect("cached cert");
		assert_eq!(got.leaf_pem, fixture_cert().leaf_pem);
	}

	#[test]
	fn directory_url_scope_is_16_hex_chars() {
		let s = directory_url_scope("https://acme-v02.api.letsencrypt.org/directory");
		assert_eq!(s.len(), 16);
		assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
	}

	#[test]
	fn dns_challenge_name_prepends_acme_challenge_label() {
		assert_eq!(dns_challenge_name("api.example.com"), "_acme-challenge.api.example.com");
	}

	#[test]
	fn dns_challenge_name_strips_wildcard_prefix() {
		// ACME servers strip `*.` before emitting the authz
		// identifier; the defensive strip here is for forward
		// compatibility if a future server breaks the convention.
		assert_eq!(dns_challenge_name("*.example.com"), "_acme-challenge.example.com");
	}

	#[test]
	fn map_dns_error_renders_with_provider_context() {
		let err = map_dns_error(&super::super::DnsProviderError::ZoneNotFound("example.com".into()));
		match err {
			RegistryError::Acme(s) => assert!(s.contains("example.com"), "{s}"),
			other => panic!("expected Acme, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn dns_cleanup_guard_drains_tracked_names_on_cleanup_now() {
		// Verify the success-path cleanup actually calls delete_txt
		// for every tracked name. Uses an Arc<RecordingDns> the
		// guard can drain through the trait object, plus a check
		// that the guard's internal list is empty after drain so
		// the Drop impl is a no-op.
		use std::sync::atomic::{AtomicUsize, Ordering};

		#[derive(Debug)]
		struct RecordingDns {
			delete_count: AtomicUsize,
		}

		#[async_trait::async_trait]
		impl super::super::DnsProvider for RecordingDns {
			async fn set_txt(&self, _: &str, _: &str) -> Result<(), super::super::DnsProviderError> {
				Ok(())
			}
			async fn delete_txt(&self, _: &str) -> Result<(), super::super::DnsProviderError> {
				self.delete_count.fetch_add(1, Ordering::SeqCst);
				Ok(())
			}
			async fn wait_propagated(
				&self,
				_: &str,
				_: &str,
				_: Duration,
			) -> Result<(), super::super::DnsProviderError> {
				Ok(())
			}
		}

		let dns = Arc::new(RecordingDns { delete_count: AtomicUsize::new(0) });
		let guard = DnsCleanupGuard::new(Arc::clone(&dns) as Arc<dyn super::super::DnsProvider>);
		guard.track("_acme-challenge.a.example".into());
		guard.track("_acme-challenge.b.example".into());
		guard.cleanup_now().await;
		assert_eq!(dns.delete_count.load(Ordering::SeqCst), 2);
		// After cleanup_now the internal list is empty; subsequent
		// drop must not call delete_txt again. Drop the guard and
		// check the count stays at 2.
		drop(guard);
		assert_eq!(dns.delete_count.load(Ordering::SeqCst), 2);
	}

	#[test]
	fn split_leaf_chain_separates_two_certs() {
		let pem = format!(
			"{}{}",
			"-----BEGIN CERTIFICATE-----\nleaf\n-----END CERTIFICATE-----\n",
			"-----BEGIN CERTIFICATE-----\nintermediate\n-----END CERTIFICATE-----\n",
		);
		let (leaf, chain) = split_leaf_chain(&pem);
		assert!(leaf.contains("leaf"));
		assert!(chain.contains("intermediate"));
	}

	#[test]
	fn split_leaf_chain_returns_empty_chain_on_single_cert() {
		let pem = "-----BEGIN CERTIFICATE-----\nleaf\n-----END CERTIFICATE-----\n";
		let (leaf, chain) = split_leaf_chain(pem);
		assert_eq!(leaf, pem);
		assert!(chain.is_empty());
	}

	#[test]
	fn generate_ecdsa_p256_csr_round_trip_through_rcgen() {
		let (key_pem, csr_der) = generate_ecdsa_p256_csr("api.example.com").expect("rcgen ok");
		assert!(key_pem.contains("-----BEGIN PRIVATE KEY-----"), "{key_pem}");
		assert!(!csr_der.is_empty());
		// The CSR should be a valid DER-encoded PKCS #10 — a
		// well-formed CSR always starts with the SEQUENCE tag 0x30.
		assert_eq!(csr_der[0], 0x30, "CSR DER must start with SEQUENCE tag");
	}

	#[test]
	fn parse_not_after_pem_extracts_validity_end() {
		// Generate a self-signed cert with rcgen so we have a known
		// PEM whose notAfter we can recover.
		let mut params =
			rcgen::CertificateParams::new(vec!["test.example".to_owned()]).expect("params");
		// rcgen 0.14 defaults `not_after` to "well in the future";
		// we just need parse_not_after_pem to return *some*
		// reasonable timestamp, so accept whatever rcgen picks.
		let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
		params.distinguished_name.push(rcgen::DnType::CommonName, "test");
		let issued = params.self_signed(&key_pair).expect("self-signed");
		let pem = issued.pem();
		let not_after = parse_not_after_pem(&pem).expect("parse");
		// Sanity: the cert's notAfter must be in the future
		// relative to the test's wall-clock.
		assert!(
			not_after > std::time::SystemTime::now(),
			"not_after {not_after:?} should be in the future",
		);
	}

	#[test]
	fn map_acme_error_classifies_rate_limited_problem() {
		let problem = instant_acme::Problem {
			r#type: Some("urn:ietf:params:acme:error:rateLimited".to_owned()),
			detail: Some("too many orders".to_owned()),
			status: Some(429),
			subproblems: Vec::new(),
		};
		let err = instant_acme::Error::Api(problem);
		match map_acme_error(err) {
			RegistryError::RateLimited { .. } => {}
			other => panic!("expected RateLimited, got {other:?}"),
		}
	}

	#[test]
	fn map_acme_error_passes_through_non_rate_limited_problems() {
		let problem = instant_acme::Problem {
			r#type: Some("urn:ietf:params:acme:error:malformed".to_owned()),
			detail: Some("nope".to_owned()),
			status: Some(400),
			subproblems: Vec::new(),
		};
		let err = instant_acme::Error::Api(problem);
		match map_acme_error(err) {
			RegistryError::Acme(_) => {}
			other => panic!("expected Acme, got {other:?}"),
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
