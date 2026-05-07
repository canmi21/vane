//! `AcmeStore` trait + the value types it persists.
//!
//! Per `spec/acme.md` § _`AcmeStore` trait_. The trait is abstract
//! over the storage backend so an alternative (object store, secrets
//! manager) can drop in without touching the registry; the default
//! [`super::FsAcmeStore`] is the one shipped in Stage 1.

use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Persistence abstraction for ACME state — accounts (per directory
/// URL) and issued certs (per SNI). Implementations must be safe to
/// share across tokio tasks; [`Self::lock`] is the serialisation
/// primitive available to operators-of-the-trait.
///
/// # Lifecycle
///
/// - `load_account` / `save_account` round-trip an [`AcmeAccount`].
///   Save is atomic from the caller's perspective — partial writes
///   never observable on subsequent loads.
/// - `load_cert` / `save_cert` likewise atomic, keyed by SNI
///   (lowercased; wildcard `*` mapped to `_wild_` for fs safety).
/// - `list_cert_snis` returns the SNIs that currently have a saved
///   cert. Used by [`super::registry::ManagedCertRegistry::open`]
///   for boot-time hydration.
/// - `lock` returns a `LockGuard` boxed-future that releases the
///   advisory lock for `scope` on drop. Different scopes don't block
///   each other; the same scope serialises across tasks **and**
///   processes (the default impl uses `flock(2)`).
///
/// Note vs `spec/acme.md`: the spec sketched a closure-taking
/// `with_lock<F, T>` shape; that signature has generic type
/// parameters which makes the trait `dyn`-incompatible. The
/// registry holds an `Arc<dyn AcmeStore>` per
/// `spec/acme.md` § _Architecture_, so this implementation uses the
/// equivalent guard-based `lock` method instead. Spec text updated
/// in this commit.
#[async_trait]
pub trait AcmeStore: Send + Sync {
	/// Fetch the persisted account for `directory_url`, or `None`
	/// when the directory has never been used. The returned
	/// account's `key_jwk` is the verbatim `instant-acme`
	/// `AccountCredentials` JSON — pass it to
	/// `AccountBuilder::from_credentials`.
	///
	/// # Errors
	/// `StoreError::Io` for filesystem failures; `StoreError::Decode`
	/// for malformed `account.json`.
	async fn load_account(&self, directory_url: &str) -> Result<Option<AcmeAccount>, StoreError>;

	/// Persist the account material. Atomic from the caller's view:
	/// either the new state lands or the prior state remains; no
	/// torn-write window. Set permission bits per
	/// `spec/acme.md` § _Storage layout_ (private).
	///
	/// # Errors
	/// `StoreError::Io` for filesystem failures; `StoreError::Encode`
	/// for serialisation failures.
	async fn save_account(
		&self,
		directory_url: &str,
		account: &AcmeAccount,
	) -> Result<(), StoreError>;

	/// Fetch the persisted cert for `sni` (lowercased), or `None`
	/// when no cert has been issued yet.
	///
	/// # Errors
	/// As above for I/O / decode errors.
	async fn load_cert(&self, sni: &str) -> Result<Option<StoredCert>, StoreError>;

	/// Persist a freshly-issued or freshly-renewed cert. Atomic:
	/// `cert.pem`, `key.pem`, and `meta.json` all land or none land
	/// (best-effort across multiple files — see fs impl for nuance).
	///
	/// # Errors
	/// As above.
	async fn save_cert(&self, sni: &str, cert: &StoredCert) -> Result<(), StoreError>;

	/// Enumerate every SNI the store currently has a saved cert for.
	/// Order is implementation-defined (the fs impl returns sorted).
	///
	/// # Errors
	/// `StoreError::Io` for filesystem failures.
	async fn list_cert_snis(&self) -> Result<Vec<String>, StoreError>;

	/// Acquire an exclusive advisory lock for `scope`. The returned
	/// guard releases the lock on drop (RAII). Different `scope`
	/// strings are independent; the same `scope` serialises across
	/// both async tasks **and** OS processes.
	///
	/// Idiomatic use:
	///
	/// ```ignore
	/// let _guard = store.lock("cert/api.example.com").await?;
	/// // … critical section: read-modify-write the cert files …
	/// // _guard drops here; the lock is released.
	/// ```
	///
	/// # Errors
	/// `StoreError::Locked` if the lock cannot be acquired;
	/// `StoreError::Io` for filesystem failures opening the lock
	/// file.
	async fn lock(&self, scope: &str) -> Result<Box<dyn LockGuard>, StoreError>;
}

/// Marker trait for an RAII handle returned by
/// [`AcmeStore::lock`]. The guard releases the lock when it goes
/// out of scope; the trait body is empty because consumers just
/// hold the handle for its destructor side effect.
///
/// `Send + Sync` so the guard can cross await points and be parked
/// in a future passed to `tokio::spawn`.
pub trait LockGuard: Send + Sync + std::fmt::Debug {}

/// Persisted ACME account material. The `key_jwk` field is the
/// verbatim `instant-acme` `AccountCredentials` serialised as JSON;
/// reload reconstructs the live `Account` via
/// `AccountBuilder::from_credentials`.
///
/// `agreed_tos_at` is recorded at registration time and surfaces in
/// `get_certs` for operator audit; spec § _Account key strategy_
/// requires CA-side ToS-version bumps to be re-acknowledged
/// explicitly through a config update + reload.
#[derive(Debug, Clone)]
pub struct AcmeAccount {
	pub directory_url: String,
	pub key_jwk: serde_json::Value,
	pub kid: String,
	pub contacts: Vec<String>,
	pub agreed_tos_at: SystemTime,
}

/// Persisted cert state. `leaf_pem` is the leaf certificate;
/// `chain_pem` is the intermediates that the leaf was issued under
/// (zero or more, concatenated). `key_pem` is the PKCS#8-PEM private
/// key matching `leaf_pem`.
///
/// `ari_replacement_id` is RFC 9773's hint for paired-renewal —
/// `None` until ARI is wired (Stage 3); `last_renew_at` is set to
/// the issuance time on first save and updated on each successful
/// renewal so the renewal scheduler has an idempotent reference.
#[derive(Debug, Clone)]
pub struct StoredCert {
	pub leaf_pem: String,
	pub chain_pem: String,
	pub key_pem: String,
	pub not_after: SystemTime,
	pub ari_replacement_id: Option<String>,
	pub last_renew_at: SystemTime,
}

/// On-disk JSON shape for [`AcmeAccount`]. Versioned so future
/// schema migrations don't silently corrupt old stores.
#[derive(Serialize, Deserialize)]
pub(super) struct AccountFileV1 {
	pub version: u32,
	pub directory_url: String,
	pub key_jwk: serde_json::Value,
	pub kid: String,
	pub contacts: Vec<String>,
	pub agreed_tos_at_unix_ms: u64,
}

impl AccountFileV1 {
	pub(super) const VERSION: u32 = 1;

	pub(super) fn from_account(a: &AcmeAccount) -> Self {
		Self {
			version: Self::VERSION,
			directory_url: a.directory_url.clone(),
			key_jwk: a.key_jwk.clone(),
			kid: a.kid.clone(),
			contacts: a.contacts.clone(),
			agreed_tos_at_unix_ms: system_time_to_unix_ms(a.agreed_tos_at),
		}
	}

	pub(super) fn into_account(self) -> AcmeAccount {
		AcmeAccount {
			directory_url: self.directory_url,
			key_jwk: self.key_jwk,
			kid: self.kid,
			contacts: self.contacts,
			agreed_tos_at: unix_ms_to_system_time(self.agreed_tos_at_unix_ms),
		}
	}
}

/// On-disk JSON shape for [`StoredCert`]'s metadata. The PEM bodies
/// (`cert.pem`, `key.pem`) live in their own files alongside this
/// `meta.json` so `cat key.pem` works from a shell session.
#[derive(Serialize, Deserialize)]
pub(super) struct CertMetaV1 {
	pub version: u32,
	pub not_after_unix_ms: u64,
	pub last_renew_at_unix_ms: u64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub ari_replacement_id: Option<String>,
}

impl CertMetaV1 {
	pub(super) const VERSION: u32 = 1;
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
	#[error("io: {0}")]
	Io(#[from] std::io::Error),
	#[error("decode: {0}")]
	Decode(String),
	#[error("encode: {0}")]
	Encode(String),
	#[error("locked: {0}")]
	Locked(String),
}

pub(super) fn system_time_to_unix_ms(t: SystemTime) -> u64 {
	// Pre-1970 timestamps round to 0 ms — the ACME spec doesn't
	// produce them and the registry never constructs negative
	// instants, so flooring is safe.
	t.duration_since(SystemTime::UNIX_EPOCH)
		.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

pub(super) fn unix_ms_to_system_time(ms: u64) -> SystemTime {
	SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn unix_ms_round_trips() {
		let now = SystemTime::now();
		let ms = system_time_to_unix_ms(now);
		let back = unix_ms_to_system_time(ms);
		// Allow up to 1ms of round-trip loss (we floor sub-ms).
		let diff = now.duration_since(back).unwrap_or_else(|e| e.duration());
		assert!(diff.as_millis() <= 1, "diff = {diff:?}");
	}

	#[test]
	fn account_file_v1_round_trips_through_json() {
		let original = AcmeAccount {
			directory_url: "https://acme-staging-v02.api.letsencrypt.org/directory".into(),
			key_jwk: serde_json::json!({"kty": "EC", "crv": "P-256"}),
			kid: "https://acme-staging-v02.api.letsencrypt.org/acme/acct/123".into(),
			contacts: vec!["mailto:ops@example.com".into()],
			agreed_tos_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
		};
		let file = AccountFileV1::from_account(&original);
		let json = serde_json::to_string(&file).expect("serialize");
		let decoded: AccountFileV1 = serde_json::from_str(&json).expect("deserialize");
		assert_eq!(decoded.version, AccountFileV1::VERSION);
		let back = decoded.into_account();
		assert_eq!(back.directory_url, original.directory_url);
		assert_eq!(back.kid, original.kid);
		assert_eq!(back.contacts, original.contacts);
		assert_eq!(back.agreed_tos_at, original.agreed_tos_at);
		assert_eq!(back.key_jwk, original.key_jwk);
	}

	#[test]
	fn cert_meta_v1_omits_ari_when_absent() {
		let meta = CertMetaV1 {
			version: CertMetaV1::VERSION,
			not_after_unix_ms: 1_700_000_000_000,
			last_renew_at_unix_ms: 1_690_000_000_000,
			ari_replacement_id: None,
		};
		let json = serde_json::to_string(&meta).expect("serialize");
		assert!(!json.contains("ari_replacement_id"), "{json}");
	}
}
