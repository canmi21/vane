//! Disk-backed default `AcmeStore` per `spec/acme.md`
//! § _Storage layout (default `FsAcmeStore`)_.
//!
//! Layout:
//!
//! ```text
//! <root>/
//!   accounts/
//!     <directory_url_hash>/
//!       account.json     # AcmeAccount, mode 0600
//!       .lock
//!   certs/
//!     <sni>/
//!       cert.pem         # leaf + intermediates, mode 0644
//!       key.pem          # PKCS#8 PEM, mode 0600
//!       meta.json        # not_after, last_renew_at, ari_replacement_id
//!       .lock
//! ```
//!
//! Permissions: `accounts/` is `0700`; private keys are `0600`;
//! everything else is `0644`. Directory traversal is protected by
//! disallowing `..` / `/` in the SNI sanitiser.
//!
//! Atomicity: every save writes to a sibling `*.tmp` file, fsyncs
//! the bytes, then `rename(2)`s into place — `cert.pem` /
//! `key.pem` / `meta.json` are written one-by-one, but each
//! individual write is crash-safe. Cross-file consistency is
//! achieved by holding the per-cert advisory lock for the entire
//! save.
//!
//! Concurrency: [`Self::lock`] combines a per-process
//! `tokio::sync::Mutex` (different scopes => different mutexes)
//! with `flock(2)` on the on-disk `.lock` file (cross-process). The
//! per-process mutex closes the well-known same-process flock gap
//! on Linux where flock is per-FD rather than per-process.

use std::collections::HashMap;
use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use fs4::FileExt;
use parking_lot::Mutex as ParkingMutex;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard};

use super::store::{
	AccountFileV1, AcmeAccount, AcmeStore, CertMetaV1, CertMetaV2, CertMetaVersionProbe, LockGuard,
	StoreError, StoredCert, system_time_to_unix_ms, unix_ms_to_system_time,
};

const ACCOUNTS_DIR: &str = "accounts";
const CERTS_DIR: &str = "certs";
const ACCOUNT_FILE: &str = "account.json";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";
const META_FILE: &str = "meta.json";
const LOCK_FILE: &str = ".lock";
/// Sibling of `cert.pem` carrying the cached OCSP staple as raw DER.
/// Distinct file (rather than base64-in-`meta.json`) because the
/// staple is already binary and `cat ocsp.der | openssl ocsp ...` is
/// the obvious operator gesture for inspection. Permissions match
/// `cert.pem` (`MODE_PUBLIC_FILE`); the staple is not secret.
const OCSP_FILE: &str = "ocsp.der";

const MODE_PRIVATE_DIR: u32 = 0o700;
const MODE_PRIVATE_FILE: u32 = 0o600;
const MODE_PUBLIC_FILE: u32 = 0o644;

/// Filesystem-backed `AcmeStore`. Open with [`Self::open`]; the
/// `<root>/accounts` and `<root>/certs` subtrees are created on
/// first use with `0700` perms.
pub struct FsAcmeStore {
	root: PathBuf,
	/// Per-scope process-local mutexes. Different scopes hash to
	/// different mutexes; the same scope serialises across tokio
	/// tasks. Combined with `flock` for cross-process exclusion.
	scope_mutexes: ParkingMutex<HashMap<String, Arc<TokioMutex<()>>>>,
}

impl FsAcmeStore {
	/// Open or create the store at `root`. Top-level `accounts/` and
	/// `certs/` directories are created with mode 0700 if missing.
	///
	/// # Errors
	/// `StoreError::Io` if the path can't be created or chmod fails.
	pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
		let root = root.into();
		std::fs::create_dir_all(&root)?;
		ensure_dir_mode(&root, MODE_PRIVATE_DIR)?;
		let accounts = root.join(ACCOUNTS_DIR);
		std::fs::create_dir_all(&accounts)?;
		ensure_dir_mode(&accounts, MODE_PRIVATE_DIR)?;
		let certs = root.join(CERTS_DIR);
		std::fs::create_dir_all(&certs)?;
		ensure_dir_mode(&certs, MODE_PRIVATE_DIR)?;
		Ok(Self { root, scope_mutexes: ParkingMutex::new(HashMap::new()) })
	}

	/// Lookup or create the per-scope process mutex. Held briefly
	/// just to clone the inner `Arc<TokioMutex<()>>`.
	fn scope_mutex(&self, scope: &str) -> Arc<TokioMutex<()>> {
		let mut map = self.scope_mutexes.lock();
		map.entry(scope.to_owned()).or_insert_with(|| Arc::new(TokioMutex::new(()))).clone()
	}

	fn account_dir(&self, directory_url: &str) -> PathBuf {
		self.root.join(ACCOUNTS_DIR).join(directory_url_hash(directory_url))
	}

	fn cert_dir(&self, sni: &str) -> PathBuf {
		self.root.join(CERTS_DIR).join(sanitise_sni(sni))
	}

	/// Translate an opaque [`AcmeStore::lock`] scope into the on-disk lock
	/// file path. The fs layout pins lock files alongside their
	/// data, so we recognise the two scope shapes the registry
	/// uses; unknown scopes get a `.lock` file under `<root>/locks/`
	/// to avoid escaping the store root.
	fn lock_path_for_scope(&self, scope: &str) -> PathBuf {
		if let Some(rest) = scope.strip_prefix("account/") {
			self.root.join(ACCOUNTS_DIR).join(rest).join(LOCK_FILE)
		} else if let Some(rest) = scope.strip_prefix("cert/") {
			self.root.join(CERTS_DIR).join(sanitise_sni(rest)).join(LOCK_FILE)
		} else {
			self.root.join("locks").join(format!("{}.lock", sanitise_scope(scope)))
		}
	}
}

#[async_trait]
impl AcmeStore for FsAcmeStore {
	async fn load_account(&self, directory_url: &str) -> Result<Option<AcmeAccount>, StoreError> {
		let path = self.account_dir(directory_url).join(ACCOUNT_FILE);
		match std::fs::read(&path) {
			Ok(bytes) => {
				let file: AccountFileV1 = serde_json::from_slice(&bytes)
					.map_err(|e| StoreError::Decode(format!("{}: {e}", path.display())))?;
				Ok(Some(file.into_account()))
			}
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
			Err(e) => Err(StoreError::Io(e)),
		}
	}

	async fn save_account(
		&self,
		directory_url: &str,
		account: &AcmeAccount,
	) -> Result<(), StoreError> {
		let dir = self.account_dir(directory_url);
		std::fs::create_dir_all(&dir)?;
		ensure_dir_mode(&dir, MODE_PRIVATE_DIR)?;
		let file = AccountFileV1::from_account(account);
		let bytes = serde_json::to_vec_pretty(&file).map_err(|e| StoreError::Encode(format!("{e}")))?;
		atomic_write_file(&dir.join(ACCOUNT_FILE), &bytes, MODE_PRIVATE_FILE)?;
		Ok(())
	}

	async fn load_cert(&self, sni: &str) -> Result<Option<StoredCert>, StoreError> {
		let dir = self.cert_dir(sni);
		let cert_path = dir.join(CERT_FILE);
		let key_path = dir.join(KEY_FILE);
		let meta_path = dir.join(META_FILE);
		let ocsp_path = dir.join(OCSP_FILE);
		match (
			std::fs::read_to_string(&cert_path),
			std::fs::read_to_string(&key_path),
			std::fs::read(&meta_path),
		) {
			(Ok(cert_chain_pem), Ok(key_pem), Ok(meta_bytes)) => {
				// Read the version field first so we can dispatch to
				// the right meta-shape variant. Old (v1) stores
				// upgrade transparently with OCSP fields = None.
				let probe: CertMetaVersionProbe = serde_json::from_slice(&meta_bytes)
					.map_err(|e| StoreError::Decode(format!("{}: {e}", meta_path.display())))?;
				let (leaf_pem, chain_pem) = split_leaf_chain(&cert_chain_pem);
				let stored = match probe.version {
					1 => {
						let meta: CertMetaV1 = serde_json::from_slice(&meta_bytes)
							.map_err(|e| StoreError::Decode(format!("{}: {e}", meta_path.display())))?;
						StoredCert {
							leaf_pem,
							chain_pem,
							key_pem,
							not_after: unix_ms_to_system_time(meta.not_after_unix_ms),
							ari_replacement_id: meta.ari_replacement_id,
							last_renew_at: unix_ms_to_system_time(meta.last_renew_at_unix_ms),
							ocsp_response: None,
							ocsp_next_update: None,
							ocsp_aia_url: None,
						}
					}
					2 => {
						let meta: CertMetaV2 = serde_json::from_slice(&meta_bytes)
							.map_err(|e| StoreError::Decode(format!("{}: {e}", meta_path.display())))?;
						// `ocsp.der` is optional — a v2 store can have
						// `ocsp_aia_url` set (cached for later fetch
						// retries) without `ocsp.der` actually existing
						// (last fetch failed).
						let ocsp_response = match std::fs::read(&ocsp_path) {
							Ok(bytes) => Some(bytes),
							Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
							Err(e) => return Err(StoreError::Io(e)),
						};
						StoredCert {
							leaf_pem,
							chain_pem,
							key_pem,
							not_after: unix_ms_to_system_time(meta.not_after_unix_ms),
							ari_replacement_id: meta.ari_replacement_id,
							last_renew_at: unix_ms_to_system_time(meta.last_renew_at_unix_ms),
							ocsp_response,
							ocsp_next_update: meta.ocsp_next_update_unix_ms.map(unix_ms_to_system_time),
							ocsp_aia_url: meta.ocsp_aia_url,
						}
					}
					other => {
						return Err(StoreError::Decode(format!(
							"{}: unknown meta version {other}",
							meta_path.display(),
						)));
					}
				};
				Ok(Some(stored))
			}
			(Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e))
				if e.kind() == std::io::ErrorKind::NotFound =>
			{
				Ok(None)
			}
			(Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => Err(StoreError::Io(e)),
		}
	}

	async fn save_cert(&self, sni: &str, cert: &StoredCert) -> Result<(), StoreError> {
		let dir = self.cert_dir(sni);
		std::fs::create_dir_all(&dir)?;
		ensure_dir_mode(&dir, MODE_PRIVATE_DIR)?;
		// `cert.pem` carries leaf + intermediate chain in spec order.
		// `chain_pem` may legitimately be empty (cross-signed root,
		// self-signed test cert); join with a trailing newline so PEM
		// parsers don't choke on the boundary.
		let cert_combined = if cert.chain_pem.is_empty() {
			cert.leaf_pem.clone()
		} else if cert.leaf_pem.ends_with('\n') {
			format!("{}{}", cert.leaf_pem, cert.chain_pem)
		} else {
			format!("{}\n{}", cert.leaf_pem, cert.chain_pem)
		};
		atomic_write_file(&dir.join(CERT_FILE), cert_combined.as_bytes(), MODE_PUBLIC_FILE)?;
		atomic_write_file(&dir.join(KEY_FILE), cert.key_pem.as_bytes(), MODE_PRIVATE_FILE)?;
		let meta = CertMetaV2 {
			version: CertMetaV2::VERSION,
			not_after_unix_ms: system_time_to_unix_ms(cert.not_after),
			last_renew_at_unix_ms: system_time_to_unix_ms(cert.last_renew_at),
			ari_replacement_id: cert.ari_replacement_id.clone(),
			ocsp_next_update_unix_ms: cert.ocsp_next_update.map(system_time_to_unix_ms),
			ocsp_aia_url: cert.ocsp_aia_url.clone(),
		};
		let meta_bytes =
			serde_json::to_vec_pretty(&meta).map_err(|e| StoreError::Encode(format!("{e}")))?;
		atomic_write_file(&dir.join(META_FILE), &meta_bytes, MODE_PUBLIC_FILE)?;
		// OCSP staple sidecar: write or remove to match the in-memory
		// state. A renewal that loses its OCSP staple (responder
		// unreachable, refresh hasn't run yet) must remove the stale
		// `ocsp.der` so the next load doesn't surface a staple bound
		// to the prior cert.
		let ocsp_path = dir.join(OCSP_FILE);
		match &cert.ocsp_response {
			Some(bytes) => atomic_write_file(&ocsp_path, bytes, MODE_PUBLIC_FILE)?,
			None => match std::fs::remove_file(&ocsp_path) {
				Ok(()) => {}
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
				Err(e) => return Err(StoreError::Io(e)),
			},
		}
		Ok(())
	}

	async fn list_cert_snis(&self) -> Result<Vec<String>, StoreError> {
		let dir = self.root.join(CERTS_DIR);
		let mut out = Vec::new();
		match std::fs::read_dir(&dir) {
			Ok(iter) => {
				for entry in iter {
					let entry = entry?;
					let file_name = entry.file_name();
					let Some(name) = file_name.to_str() else { continue };
					out.push(unsanitise_sni(name));
				}
			}
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
			Err(e) => return Err(StoreError::Io(e)),
		}
		out.sort();
		Ok(out)
	}

	async fn lock(&self, scope: &str) -> Result<Box<dyn LockGuard>, StoreError> {
		// Step 1: per-process serialisation via tokio::sync::Mutex so
		// `.lock().await` doesn't block the runtime worker. We need
		// `OwnedMutexGuard` because the guard outlives this scope —
		// it travels back to the caller as part of the boxed
		// `LockGuard`.
		let mutex = self.scope_mutex(scope);
		let proc_guard: OwnedMutexGuard<()> = Arc::clone(&mutex).lock_owned().await;

		// Step 2: cross-process flock. Open the on-disk lock file
		// (creating it + its parents if needed) and hold an
		// exclusive flock for the lifetime of the returned guard.
		// `spawn_blocking` keeps the (potentially blocking) flock
		// syscall off the runtime's workers.
		let lock_path = self.lock_path_for_scope(scope);
		if let Some(parent) = lock_path.parent() {
			std::fs::create_dir_all(parent)?;
		}
		let lock_file =
			OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock_path)?;
		let lock_file = tokio::task::spawn_blocking(move || {
			// fs4 1.1 names the exclusive-blocking lock just `lock` —
			// the `_exclusive` suffix from earlier major versions
			// folded back into the default behaviour.
			FileExt::lock(&lock_file).map(|()| lock_file)
		})
		.await
		.map_err(|e| StoreError::Locked(format!("flock task panicked: {e}")))??;

		Ok(Box::new(FsLockGuard { _proc_guard: proc_guard, file: Some(lock_file) }))
	}
}

/// RAII guard returned by [`FsAcmeStore::lock`]. Holds the
/// per-process tokio `OwnedMutexGuard` and the OS-level flock'd
/// file; both release on drop.
struct FsLockGuard {
	_proc_guard: OwnedMutexGuard<()>,
	file: Option<File>,
}

impl std::fmt::Debug for FsLockGuard {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		// `OwnedMutexGuard<()>` carries no useful debug payload, and
		// `File` would print the OS file descriptor — neither helps a
		// reader. Surface the held-state of the flock instead, which
		// is what reviewers actually want to know about a lock guard.
		// `finish_non_exhaustive` signals the omitted fields are
		// intentional and silences clippy's manual-Debug lint.
		f.debug_struct("FsLockGuard").field("flock_held", &self.file.is_some()).finish_non_exhaustive()
	}
}

impl Drop for FsLockGuard {
	fn drop(&mut self) {
		if let Some(file) = self.file.take() {
			// `flock` is per-FD on most platforms; closing the file
			// also releases the lock, but be explicit so the
			// release-on-drop semantic is verifiable in code review.
			let _ = file.unlock();
		}
	}
}

impl LockGuard for FsLockGuard {}

/// `sha256(directory_url)[..16]` — first 16 hex chars per
/// `spec/acme.md` § _Storage layout_. Keeps multi-CA support open
/// without committing to an explicit multi-CA schema today.
fn directory_url_hash(directory_url: &str) -> String {
	use std::fmt::Write as _;
	let digest = Sha256::digest(directory_url.as_bytes());
	let hex = digest.iter().fold(String::with_capacity(64), |mut acc, b| {
		let _ = write!(acc, "{b:02x}");
		acc
	});
	hex.chars().take(16).collect()
}

/// SNI → filesystem-safe directory name. Lowercased per
/// 08-tls.md § _SNI normalization_; `*` → `_wild_` so wildcard
/// SANs don't end up creating shell-glob hazards.
fn sanitise_sni(sni: &str) -> String {
	let lower = sni.to_ascii_lowercase();
	lower.replace('*', "_wild_")
}

/// Inverse of [`sanitise_sni`] — used by `list_cert_snis` to
/// reconstruct the operator-visible SNI from the on-disk dir name.
fn unsanitise_sni(name: &str) -> String {
	name.replace("_wild_", "*")
}

/// Generic scope sanitiser for unknown `with_lock` scopes (those
/// that don't match the `account/` / `cert/` prefixes). Replaces
/// path separators and dot-runs with underscores so a malicious
/// scope can't escape `<root>/locks/`.
fn sanitise_scope(scope: &str) -> String {
	scope
		.chars()
		.map(|c| match c {
			'/' | '\\' | '\0' => '_',
			c if c.is_control() => '_',
			c => c,
		})
		.collect::<String>()
		.replace("..", "__")
}

/// Atomic write: `tmp` file + fsync + `rename(2)`. Sets `mode` on
/// the destination after rename so the visible file always has the
/// intended permissions.
fn atomic_write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), StoreError> {
	let parent =
		path.parent().ok_or_else(|| StoreError::Io(std::io::Error::other("path has no parent")))?;
	let file_name = path
		.file_name()
		.ok_or_else(|| StoreError::Io(std::io::Error::other("path has no file name")))?;
	let tmp = parent.join(format!(".{}.tmp", file_name.to_string_lossy()));
	{
		let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
		f.write_all(bytes)?;
		f.sync_all()?;
	}
	std::fs::rename(&tmp, path)?;
	// `rename` preserves perms across the create site, but on some
	// fs the new inode might inherit umask-applied perms; chmod
	// post-rename to lock in the intended mode.
	std::fs::set_permissions(path, Permissions::from_mode(mode))?;
	// Also fsync the parent dir so the rename itself is durable.
	if let Ok(dir) = File::open(parent) {
		let _ = dir.sync_all();
	}
	Ok(())
}

fn ensure_dir_mode(path: &Path, mode: u32) -> Result<(), StoreError> {
	std::fs::set_permissions(path, Permissions::from_mode(mode))?;
	Ok(())
}

/// Split a `leaf+chain` PEM blob into the leaf certificate PEM and
/// the rest. The PEM grammar is line-oriented; we look for the
/// second `-----BEGIN CERTIFICATE-----` to find the cut point.
fn split_leaf_chain(pem: &str) -> (String, String) {
	const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
	let mut iter = pem.match_indices(BEGIN);
	let _first = iter.next();
	match iter.next() {
		Some((idx, _)) => (pem[..idx].to_owned(), pem[idx..].to_owned()),
		None => (pem.to_owned(), String::new()),
	}
}

#[cfg(test)]
mod tests {
	use std::time::{Duration, SystemTime};

	use tempfile::TempDir;

	use super::*;

	fn fixture_account() -> AcmeAccount {
		AcmeAccount {
			directory_url: "https://acme-staging-v02.api.letsencrypt.org/directory".into(),
			key_jwk: serde_json::json!({"kty": "EC", "crv": "P-256", "d": "xxx"}),
			kid: "https://acme-staging-v02.api.letsencrypt.org/acme/acct/42".into(),
			contacts: vec!["mailto:ops@example.com".into()],
			agreed_tos_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
		}
	}

	fn fixture_cert() -> StoredCert {
		StoredCert {
			leaf_pem: "-----BEGIN CERTIFICATE-----\nLEAF\n-----END CERTIFICATE-----\n".into(),
			chain_pem: "-----BEGIN CERTIFICATE-----\nCHAIN\n-----END CERTIFICATE-----\n".into(),
			key_pem: "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n".into(),
			not_after: SystemTime::UNIX_EPOCH + Duration::from_hours(500_000),
			ari_replacement_id: None,
			last_renew_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
			ocsp_response: None,
			ocsp_next_update: None,
			ocsp_aia_url: None,
		}
	}

	#[tokio::test]
	async fn account_round_trips() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let acct = fixture_account();
		store.save_account(&acct.directory_url, &acct).await.unwrap();
		let back = store.load_account(&acct.directory_url).await.unwrap().unwrap();
		assert_eq!(back.directory_url, acct.directory_url);
		assert_eq!(back.kid, acct.kid);
		assert_eq!(back.contacts, acct.contacts);
		assert_eq!(back.agreed_tos_at, acct.agreed_tos_at);
		assert_eq!(back.key_jwk, acct.key_jwk);
	}

	#[tokio::test]
	async fn account_load_returns_none_when_absent() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let back = store.load_account("https://acme-v02.api.letsencrypt.org/directory").await.unwrap();
		assert!(back.is_none());
	}

	#[tokio::test]
	async fn cert_round_trips() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let cert = fixture_cert();
		store.save_cert("api.example.com", &cert).await.unwrap();
		let back = store.load_cert("api.example.com").await.unwrap().unwrap();
		assert_eq!(back.leaf_pem, cert.leaf_pem);
		assert_eq!(back.chain_pem, cert.chain_pem);
		assert_eq!(back.key_pem, cert.key_pem);
		assert_eq!(back.not_after, cert.not_after);
		assert_eq!(back.ari_replacement_id, cert.ari_replacement_id);
		assert_eq!(back.last_renew_at, cert.last_renew_at);
		assert_eq!(back.ocsp_response, cert.ocsp_response);
		assert_eq!(back.ocsp_next_update, cert.ocsp_next_update);
		assert_eq!(back.ocsp_aia_url, cert.ocsp_aia_url);
	}

	#[tokio::test]
	async fn meta_v1_loads_with_ocsp_fields_as_none() {
		// Hand-write a v1-shaped meta.json + cert.pem + key.pem and
		// confirm load_cert hydrates StoredCert with ocsp_* = None.
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let dir = tmp.path().join(CERTS_DIR).join("legacy.example");
		std::fs::create_dir_all(&dir).unwrap();
		std::fs::write(
			dir.join(CERT_FILE),
			"-----BEGIN CERTIFICATE-----\nLEAF\n-----END CERTIFICATE-----\n",
		)
		.unwrap();
		std::fs::write(
			dir.join(KEY_FILE),
			"-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n",
		)
		.unwrap();
		std::fs::write(
			dir.join(META_FILE),
			r#"{"version":1,"not_after_unix_ms":1700000000000,"last_renew_at_unix_ms":1690000000000}"#,
		)
		.unwrap();

		let back = store.load_cert("legacy.example").await.unwrap().unwrap();
		assert!(back.ocsp_response.is_none());
		assert!(back.ocsp_next_update.is_none());
		assert!(back.ocsp_aia_url.is_none());
		assert_eq!(back.not_after, unix_ms_to_system_time(1_700_000_000_000));
	}

	#[tokio::test]
	async fn meta_v2_round_trips_with_ocsp_response() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let mut cert = fixture_cert();
		cert.ocsp_response = Some(b"\x30\x0a\x0bRAW OCSP DER".to_vec());
		// 1_800_000_000 s ≈ 2027-01 — picked to be plainly future
		// relative to the test wall-clock without sliding into the
		// `Duration::from_days` rounding range clippy prefers.
		cert.ocsp_next_update = Some(SystemTime::UNIX_EPOCH + Duration::from_hours(500_000));
		cert.ocsp_aia_url = Some("http://ocsp.example.test/".into());
		store.save_cert("api.example.com", &cert).await.unwrap();

		let back = store.load_cert("api.example.com").await.unwrap().unwrap();
		assert_eq!(back.ocsp_response, cert.ocsp_response);
		assert_eq!(back.ocsp_next_update, cert.ocsp_next_update);
		assert_eq!(back.ocsp_aia_url, cert.ocsp_aia_url);
	}

	#[tokio::test]
	async fn save_cert_removes_ocsp_der_when_response_cleared() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let mut cert = fixture_cert();
		cert.ocsp_response = Some(b"DER".to_vec());
		store.save_cert("api.example.com", &cert).await.unwrap();
		let ocsp_path = tmp.path().join(CERTS_DIR).join("api.example.com").join(OCSP_FILE);
		assert!(ocsp_path.exists(), "ocsp.der written on first save");

		// Re-save without an OCSP staple — the sidecar must be removed
		// so a subsequent load doesn't surface a stale staple.
		cert.ocsp_response = None;
		store.save_cert("api.example.com", &cert).await.unwrap();
		assert!(!ocsp_path.exists(), "ocsp.der removed when staple cleared");
	}

	#[tokio::test]
	async fn ocsp_der_perms_match_cert_pem() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let mut cert = fixture_cert();
		cert.ocsp_response = Some(b"DER".to_vec());
		store.save_cert("api.example.com", &cert).await.unwrap();
		let ocsp_path = tmp.path().join(CERTS_DIR).join("api.example.com").join(OCSP_FILE);
		let mode = std::fs::metadata(&ocsp_path).unwrap().permissions().mode() & 0o777;
		// OCSP staples are not secret — `0644` matches `cert.pem`.
		assert_eq!(mode, MODE_PUBLIC_FILE);
	}

	#[tokio::test]
	async fn cert_load_returns_none_when_absent() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		assert!(store.load_cert("nope.example.com").await.unwrap().is_none());
	}

	#[tokio::test]
	async fn list_cert_snis_returns_sorted() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let cert = fixture_cert();
		store.save_cert("zeta.example.com", &cert).await.unwrap();
		store.save_cert("alpha.example.com", &cert).await.unwrap();
		store.save_cert("mu.example.com", &cert).await.unwrap();
		let snis = store.list_cert_snis().await.unwrap();
		assert_eq!(snis, vec!["alpha.example.com", "mu.example.com", "zeta.example.com"]);
	}

	#[tokio::test]
	async fn list_cert_snis_unsanitises_wildcards() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let cert = fixture_cert();
		store.save_cert("*.example.com", &cert).await.unwrap();
		let snis = store.list_cert_snis().await.unwrap();
		assert_eq!(snis, vec!["*.example.com"]);
	}

	#[tokio::test]
	async fn list_cert_snis_returns_empty_on_fresh_store() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let snis = store.list_cert_snis().await.unwrap();
		assert!(snis.is_empty());
	}

	#[tokio::test]
	async fn corrupted_meta_json_returns_decode_error() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let cert = fixture_cert();
		store.save_cert("bad.example.com", &cert).await.unwrap();
		let meta_path = tmp.path().join(CERTS_DIR).join("bad.example.com").join(META_FILE);
		std::fs::write(&meta_path, b"{ not valid json").unwrap();
		match store.load_cert("bad.example.com").await {
			Err(StoreError::Decode(_)) => {}
			other => panic!("expected Decode, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn accounts_dir_perms_are_0700() {
		let tmp = TempDir::new().unwrap();
		let _ = FsAcmeStore::open(tmp.path()).unwrap();
		let mode =
			std::fs::metadata(tmp.path().join(ACCOUNTS_DIR)).unwrap().permissions().mode() & 0o777;
		assert_eq!(mode, MODE_PRIVATE_DIR);
	}

	#[tokio::test]
	async fn account_file_perms_are_0600() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let acct = fixture_account();
		store.save_account(&acct.directory_url, &acct).await.unwrap();
		let path = tmp
			.path()
			.join(ACCOUNTS_DIR)
			.join(directory_url_hash(&acct.directory_url))
			.join(ACCOUNT_FILE);
		let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
		assert_eq!(mode, MODE_PRIVATE_FILE);
	}

	#[tokio::test]
	async fn key_pem_perms_are_0600() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let cert = fixture_cert();
		store.save_cert("api.example.com", &cert).await.unwrap();
		let path = tmp.path().join(CERTS_DIR).join("api.example.com").join(KEY_FILE);
		let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
		assert_eq!(mode, MODE_PRIVATE_FILE);
	}

	#[tokio::test]
	async fn cert_pem_perms_are_0644() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let cert = fixture_cert();
		store.save_cert("api.example.com", &cert).await.unwrap();
		let path = tmp.path().join(CERTS_DIR).join("api.example.com").join(CERT_FILE);
		let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
		assert_eq!(mode, MODE_PUBLIC_FILE);
	}

	#[tokio::test]
	async fn save_is_idempotent_under_repeated_writes() {
		let tmp = TempDir::new().unwrap();
		let store = FsAcmeStore::open(tmp.path()).unwrap();
		let cert = fixture_cert();
		for _ in 0..3 {
			store.save_cert("api.example.com", &cert).await.unwrap();
		}
		let back = store.load_cert("api.example.com").await.unwrap().unwrap();
		assert_eq!(back.leaf_pem, cert.leaf_pem);
	}

	#[tokio::test]
	async fn lock_serialises_concurrent_same_scope_holders() {
		let tmp = TempDir::new().unwrap();
		let store: Arc<dyn AcmeStore> = Arc::new(FsAcmeStore::open(tmp.path()).unwrap());
		let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
		let parallel = Arc::new(std::sync::atomic::AtomicUsize::new(0));
		let max_parallel = Arc::new(std::sync::atomic::AtomicUsize::new(0));

		let mut handles = Vec::new();
		for _ in 0..8 {
			let store = Arc::clone(&store);
			let counter = Arc::clone(&counter);
			let parallel = Arc::clone(&parallel);
			let max_parallel = Arc::clone(&max_parallel);
			handles.push(tokio::spawn(async move {
				let _guard = store.lock("test/scope").await.unwrap();
				let n = parallel.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
				let mut prev = max_parallel.load(std::sync::atomic::Ordering::SeqCst);
				while n > prev {
					match max_parallel.compare_exchange(
						prev,
						n,
						std::sync::atomic::Ordering::SeqCst,
						std::sync::atomic::Ordering::SeqCst,
					) {
						Ok(_) => break,
						Err(actual) => prev = actual,
					}
				}
				tokio::time::sleep(Duration::from_millis(20)).await;
				parallel.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
				counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
			}));
		}
		for h in handles {
			h.await.unwrap();
		}
		assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 8);
		assert_eq!(
			max_parallel.load(std::sync::atomic::Ordering::SeqCst),
			1,
			"concurrent same-scope sections must serialise"
		);
	}

	#[tokio::test]
	async fn lock_distinct_scopes_run_concurrently() {
		let tmp = TempDir::new().unwrap();
		let store: Arc<dyn AcmeStore> = Arc::new(FsAcmeStore::open(tmp.path()).unwrap());
		let parallel = Arc::new(std::sync::atomic::AtomicUsize::new(0));
		let max_parallel = Arc::new(std::sync::atomic::AtomicUsize::new(0));

		let mut handles = Vec::new();
		for i in 0..6 {
			let store = Arc::clone(&store);
			let parallel = Arc::clone(&parallel);
			let max_parallel = Arc::clone(&max_parallel);
			let scope = format!("cert/sni-{i}");
			handles.push(tokio::spawn(async move {
				let _guard = store.lock(&scope).await.unwrap();
				let n = parallel.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
				let mut prev = max_parallel.load(std::sync::atomic::Ordering::SeqCst);
				while n > prev {
					match max_parallel.compare_exchange(
						prev,
						n,
						std::sync::atomic::Ordering::SeqCst,
						std::sync::atomic::Ordering::SeqCst,
					) {
						Ok(_) => break,
						Err(actual) => prev = actual,
					}
				}
				tokio::time::sleep(Duration::from_millis(40)).await;
				parallel.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
			}));
		}
		for h in handles {
			h.await.unwrap();
		}
		assert!(
			max_parallel.load(std::sync::atomic::Ordering::SeqCst) > 1,
			"distinct scopes should run in parallel"
		);
	}

	#[tokio::test]
	async fn lock_releases_on_guard_drop() {
		let tmp = TempDir::new().unwrap();
		let store: Arc<dyn AcmeStore> = Arc::new(FsAcmeStore::open(tmp.path()).unwrap());

		// Hold and drop a guard, then acquire again on the same
		// scope without timing out. If the previous guard didn't
		// release the lock, this would deadlock.
		{
			let _g1 = store.lock("test/release").await.unwrap();
		}
		let _g2 = tokio::time::timeout(Duration::from_millis(200), store.lock("test/release"))
			.await
			.expect("second acquire must succeed quickly")
			.expect("lock");
	}

	#[test]
	fn directory_url_hash_is_16_hex_chars() {
		let h = directory_url_hash("https://acme-v02.api.letsencrypt.org/directory");
		assert_eq!(h.len(), 16);
		assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
	}

	#[test]
	fn sanitise_sni_handles_wildcards_and_case() {
		assert_eq!(sanitise_sni("*.Example.COM"), "_wild_.example.com");
		assert_eq!(sanitise_sni("api.example.com"), "api.example.com");
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
}
