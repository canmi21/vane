//! Process-wide CRL cache keyed by source identity. CRL bytes mutate
//! in place across refresh cycles, so any surrounding `Arc<ClientConfig>`
//! / `Arc<ServerConfig>` identity stays stable.
//!
//! Wrapper verifiers in [`crate::verifier`] pull the latest snapshot
//! per handshake.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use rustls_pki_types::CertificateRevocationListDer;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

/// Source identity used as the cache key. The fingerprint hashes the
/// path / URL string, **not** the fetched bytes — so refresh cycles
/// never invalidate downstream caches keyed off this identity.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub enum CrlSourceId {
	File(PathBuf),
	Url(String),
}

impl CrlSourceId {
	#[must_use]
	pub fn from_file<P: Into<PathBuf>>(path: P) -> Self {
		Self::File(path.into())
	}

	#[must_use]
	pub fn from_url<S: Into<String>>(url: S) -> Self {
		Self::Url(url.into())
	}
}

/// Per-source policy on what to do when a CRL becomes unavailable.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CrlFetchFailure {
	/// Keep using last-known bytes; if never loaded, silently drop.
	Tolerate,
	/// Surface as a hard error from `snapshot` so handshakes fail.
	Reject,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum HealthState {
	Healthy,
	Unavailable,
}

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const FALLBACK_INTERVAL: Duration = Duration::from_hours(4);
const REFRESH_LEAD: Duration = Duration::from_hours(1);

struct CrlEntry {
	bytes: Option<Arc<CertificateRevocationListDer<'static>>>,
	next_update: Option<OffsetDateTime>,
	last_success: Option<OffsetDateTime>,
	last_failure: Option<OffsetDateTime>,
	fetch_failure: CrlFetchFailure,
	last_logged_state: HealthState,
}

/// Pluggable transport. Production wires up an HTTP / `tokio::fs`
/// fetcher; tests substitute in-memory mocks to drive failure paths
/// and rotation.
#[async_trait]
pub trait CrlFetcher: Send + Sync {
	/// Fetch the raw bytes for one source. File source: typically read
	/// from disk. URL source: typically HTTP GET. Returns DER bytes on
	/// success; PEM input is decoded by the cache via `rustls-pemfile`
	/// before parsing. Caller's `await` is timed out at 30 s.
	async fn fetch(&self, src: &CrlSourceId) -> Result<Vec<u8>, String>;
}

/// Process-wide CRL cache.
pub struct CrlCache {
	inner: RwLock<HashMap<CrlSourceId, CrlEntry>>,
	fetcher: Arc<dyn CrlFetcher>,
}

impl std::fmt::Debug for CrlCache {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let guard = self.inner.read();
		f.debug_struct("CrlCache").field("entries", &guard.len()).finish_non_exhaustive()
	}
}

impl CrlCache {
	#[must_use]
	pub fn new(fetcher: Arc<dyn CrlFetcher>) -> Arc<Self> {
		Arc::new(Self { inner: RwLock::new(HashMap::new()), fetcher })
	}

	/// Synchronous link-time loader. Each source is fetched with a
	/// 30-second timeout. On success, parses `nextUpdate` and stores
	/// the bytes. On failure, behavior depends on `policy`:
	///
	/// * [`CrlFetchFailure::Tolerate`] — record the failure and
	///   continue. Subsequent [`Self::snapshot`] calls for this source
	///   silently drop it until a refresh succeeds.
	/// * [`CrlFetchFailure::Reject`] — propagate the error so the
	///   caller can fail link.
	///
	/// # Panics
	///
	/// Must be called from within a multi-thread tokio runtime — uses
	/// `block_in_place` + `Handle::current().block_on`. Single-thread
	/// runtimes panic.
	///
	/// # Errors
	///
	/// String description of the first reject-policy source that
	/// failed to load. Tolerate-policy failures are kept silent at
	/// link time (logged as transitions, but `Ok` returned).
	pub fn ensure_loaded(&self, sources: &[(CrlSourceId, CrlFetchFailure)]) -> Result<(), String> {
		tokio::task::block_in_place(|| {
			tokio::runtime::Handle::current().block_on(async {
				for (src, policy) in sources {
					self.fetch_source(src, *policy).await?;
				}
				Ok(())
			})
		})
	}

	/// Read-only handshake-time accessor. Returns the latest CRL bytes
	/// for each requested source. Sources whose policy is `tolerate`
	/// and whose entry has never successfully loaded are silently
	/// dropped from the result. Sources whose policy is `reject` and
	/// whose entry is currently `unavailable` cause this function to
	/// return `Err` — wrappers turn that into a handshake failure.
	///
	/// # Errors
	///
	/// Returns the first reject-policy source whose state is
	/// `Unavailable`.
	pub fn snapshot(
		&self,
		sources: &[CrlSourceId],
	) -> Result<Vec<Arc<CertificateRevocationListDer<'static>>>, String> {
		let now = OffsetDateTime::now_utc();
		let guard = self.inner.read();
		let mut out = Vec::with_capacity(sources.len());
		for src in sources {
			let Some(entry) = guard.get(src) else {
				return Err(format!("crl source not registered: {src:?}"));
			};
			let state = entry_state(entry, now);
			match (state, entry.fetch_failure) {
				(HealthState::Healthy, _) => {
					if let Some(bytes) = &entry.bytes {
						out.push(Arc::clone(bytes));
					}
				}
				(HealthState::Unavailable, CrlFetchFailure::Tolerate) => {
					// `tolerate` + cached but stale: keep using the
					// last-known bytes. `tolerate` + never-loaded:
					// silently drop.
					if let Some(bytes) = &entry.bytes {
						out.push(Arc::clone(bytes));
					}
				}
				(HealthState::Unavailable, CrlFetchFailure::Reject) => {
					return Err(format!("crl source unavailable (reject policy): {src:?}"));
				}
			}
		}
		Ok(out)
	}

	/// Reload-friendly variant of [`Self::ensure_loaded`]: only fetches
	/// sources whose entry is not already registered. Useful from the
	/// reload path so an unchanged URL source doesn't re-block on a
	/// cold fetch every time the watcher fires.
	///
	/// File sources are always re-fetched (their bytes are local).
	///
	/// # Panics
	///
	/// Same multi-thread runtime requirement as [`Self::ensure_loaded`].
	///
	/// # Errors
	///
	/// As [`Self::ensure_loaded`].
	pub fn ensure_loaded_new(
		&self,
		sources: &[(CrlSourceId, CrlFetchFailure)],
	) -> Result<(), String> {
		let to_fetch: Vec<(CrlSourceId, CrlFetchFailure)> = {
			let guard = self.inner.read();
			sources
				.iter()
				.filter(|(id, _)| match id {
					CrlSourceId::File(_) => true,
					CrlSourceId::Url(_) => !guard.contains_key(id),
				})
				.cloned()
				.collect()
		};
		if to_fetch.is_empty() {
			return Ok(());
		}
		self.ensure_loaded(&to_fetch)
	}

	/// Spawn the background refresh loop. One tokio task per URL
	/// source — file sources don't refresh here (callers re-read them
	/// via [`Self::ensure_loaded`] on reload). Cancellation token lets
	/// the host stop the workers at shutdown.
	pub fn spawn_refresher(self: &Arc<Self>, shutdown: &CancellationToken) {
		let urls: Vec<CrlSourceId> = {
			let guard = self.inner.read();
			guard.keys().filter(|k| matches!(k, CrlSourceId::Url(_))).cloned().collect()
		};
		for src in urls {
			let cache = Arc::clone(self);
			let shutdown = shutdown.clone();
			tokio::spawn(async move {
				cache.refresh_loop(src, shutdown).await;
			});
		}
	}

	async fn refresh_loop(self: Arc<Self>, src: CrlSourceId, shutdown: CancellationToken) {
		loop {
			let policy = {
				let guard = self.inner.read();
				match guard.get(&src) {
					Some(e) => e.fetch_failure,
					None => return,
				}
			};
			let next_in = self.next_refresh_delay(&src);
			tokio::select! {
				() = shutdown.cancelled() => return,
				() = tokio::time::sleep(next_in) => {}
			}
			let _ = self.fetch_source(&src, policy).await;
		}
	}

	fn next_refresh_delay(&self, src: &CrlSourceId) -> Duration {
		let guard = self.inner.read();
		let Some(entry) = guard.get(src) else {
			return FALLBACK_INTERVAL;
		};
		let Some(nu) = entry.next_update else {
			return FALLBACK_INTERVAL;
		};
		let now = OffsetDateTime::now_utc();
		let target = nu - REFRESH_LEAD;
		if target <= now {
			Duration::from_secs(0)
		} else {
			let delta = target - now;
			delta.try_into().unwrap_or(FALLBACK_INTERVAL)
		}
	}

	async fn fetch_source(&self, src: &CrlSourceId, policy: CrlFetchFailure) -> Result<(), String> {
		// Insert / refresh policy on the entry up front so concurrent
		// snapshot() readers see a consistent state machine.
		{
			let mut guard = self.inner.write();
			let entry = guard.entry(src.clone()).or_insert_with(|| CrlEntry {
				bytes: None,
				next_update: None,
				last_success: None,
				last_failure: None,
				fetch_failure: policy,
				last_logged_state: HealthState::Unavailable,
			});
			entry.fetch_failure = policy;
		}

		let outcome = tokio::time::timeout(FETCH_TIMEOUT, self.fetcher.fetch(src)).await;
		let result: Result<Vec<u8>, String> = match outcome {
			Ok(r) => r,
			Err(_) => Err(format!("crl fetch timeout after {}s", FETCH_TIMEOUT.as_secs())),
		};

		// Pre-decode any PEM-armoured CRL into raw DER before parsing
		// `nextUpdate`. Callers can hand back either form.
		let result = result.map(|bytes| decode_pem_crl(&bytes).unwrap_or(bytes));

		match result {
			Ok(bytes) => {
				let next_update = parse_next_update(&bytes);
				let der: CertificateRevocationListDer<'static> = CertificateRevocationListDer::from(bytes);
				let prev_state = {
					let mut guard = self.inner.write();
					let entry = guard.get_mut(src).expect("entry inserted above");
					let prev = entry.last_logged_state;
					entry.bytes = Some(Arc::new(der));
					entry.next_update = next_update;
					entry.last_success = Some(OffsetDateTime::now_utc());
					entry.last_logged_state = HealthState::Healthy;
					prev
				};
				if prev_state == HealthState::Unavailable {
					tracing::info!(?src, "crl source recovered");
				}
				Ok(())
			}
			Err(err) => {
				let (prev_state, policy) = {
					let mut guard = self.inner.write();
					let entry = guard.get_mut(src).expect("entry inserted above");
					entry.last_failure = Some(OffsetDateTime::now_utc());
					let prev = entry.last_logged_state;
					entry.last_logged_state = HealthState::Unavailable;
					(prev, entry.fetch_failure)
				};
				if prev_state == HealthState::Healthy {
					match policy {
						CrlFetchFailure::Tolerate => {
							tracing::warn!(?src, error = %err, "crl source became unavailable; using last-known bytes");
						}
						CrlFetchFailure::Reject => {
							tracing::error!(?src, error = %err, "crl source became unavailable; reject policy will fail handshakes");
						}
					}
				}
				match policy {
					CrlFetchFailure::Tolerate => Ok(()),
					CrlFetchFailure::Reject => Err(format!("crl source {src:?}: {err}")),
				}
			}
		}
	}
}

fn entry_state(entry: &CrlEntry, now: OffsetDateTime) -> HealthState {
	let Some(_bytes) = entry.bytes.as_ref() else {
		return HealthState::Unavailable;
	};
	let Some(nu) = entry.next_update else {
		return HealthState::Healthy;
	};
	if now <= nu {
		return HealthState::Healthy;
	}
	// Stale. Unavailable iff the most recent refetch attempt failed.
	match (entry.last_success, entry.last_failure) {
		(Some(s), Some(f)) if f > s => HealthState::Unavailable,
		_ => HealthState::Healthy,
	}
}

fn parse_next_update(der: &[u8]) -> Option<OffsetDateTime> {
	use x509_parser::prelude::FromDer as _;
	let (_rest, crl) = x509_parser::revocation_list::CertificateRevocationList::from_der(der).ok()?;
	let nu = crl.tbs_cert_list.next_update?;
	nu.to_datetime().into()
}

/// Read a CRL file from disk and return raw DER bytes. PEM-armoured
/// inputs are decoded; non-PEM inputs pass through unchanged. Useful
/// for [`CrlFetcher`] implementations that back `CrlSourceId::File`.
///
/// # Errors
///
/// Wraps the underlying `tokio::fs::read` error.
pub async fn read_crl_file(path: &Path) -> Result<Vec<u8>, String> {
	let bytes =
		tokio::fs::read(path).await.map_err(|e| format!("read crl file {}: {e}", path.display()))?;
	if let Some(der) = decode_pem_crl(&bytes) {
		return Ok(der);
	}
	Ok(bytes)
}

fn decode_pem_crl(bytes: &[u8]) -> Option<Vec<u8>> {
	let mut reader = std::io::BufReader::new(bytes);
	if let Some(der) = rustls_pemfile::crls(&mut reader).flatten().next() {
		return Some(der.as_ref().to_vec());
	}
	None
}

/// Dedupe a CRL source list by [`CrlSourceId`], keeping the strictest
/// policy ([`CrlFetchFailure::Reject`] wins over
/// [`CrlFetchFailure::Tolerate`]) when the same source appears
/// multiple times. Order in the result is the first-seen order.
#[must_use]
pub fn dedupe_crl_sources(
	iter: impl IntoIterator<Item = (CrlSourceId, CrlFetchFailure)>,
) -> Vec<(CrlSourceId, CrlFetchFailure)> {
	use std::collections::HashMap;
	let mut by_id: HashMap<CrlSourceId, CrlFetchFailure> = HashMap::new();
	let mut order: Vec<CrlSourceId> = Vec::new();
	for (id, policy) in iter {
		match by_id.entry(id.clone()) {
			std::collections::hash_map::Entry::Vacant(slot) => {
				slot.insert(policy);
				order.push(id);
			}
			std::collections::hash_map::Entry::Occupied(mut slot) => {
				if matches!(policy, CrlFetchFailure::Reject) {
					slot.insert(CrlFetchFailure::Reject);
				}
			}
		}
	}
	order
		.into_iter()
		.map(|id| {
			let policy = by_id[&id];
			(id, policy)
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

	use super::*;

	struct StaticFetcher {
		bytes: Vec<u8>,
		count: AtomicUsize,
	}

	#[async_trait]
	impl CrlFetcher for StaticFetcher {
		async fn fetch(&self, _src: &CrlSourceId) -> Result<Vec<u8>, String> {
			self.count.fetch_add(1, Ordering::SeqCst);
			Ok(self.bytes.clone())
		}
	}

	struct AlwaysFailFetcher {
		count: AtomicUsize,
	}

	#[async_trait]
	impl CrlFetcher for AlwaysFailFetcher {
		async fn fetch(&self, _src: &CrlSourceId) -> Result<Vec<u8>, String> {
			self.count.fetch_add(1, Ordering::SeqCst);
			Err("fixture failure".into())
		}
	}

	struct FlippingFetcher {
		ok_bytes: Vec<u8>,
		succeed: AtomicBool,
	}

	#[async_trait]
	impl CrlFetcher for FlippingFetcher {
		async fn fetch(&self, _src: &CrlSourceId) -> Result<Vec<u8>, String> {
			if self.succeed.load(Ordering::SeqCst) {
				Ok(self.ok_bytes.clone())
			} else {
				Err("flip failure".into())
			}
		}
	}

	// Minimal CRL DER built once via rcgen. Cheap enough at test time.
	fn fixture_crl_bytes() -> Vec<u8> {
		use rcgen::{
			CertificateParams, CertificateRevocationListParams, Issuer, KeyIdMethod, KeyPair,
			KeyUsagePurpose, RevocationReason, RevokedCertParams, SerialNumber,
		};
		let mut ca_params = CertificateParams::new(vec!["fixture ca".into()]).expect("ca params");
		ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
		ca_params.key_usages = vec![
			KeyUsagePurpose::KeyCertSign,
			KeyUsagePurpose::DigitalSignature,
			KeyUsagePurpose::CrlSign,
		];
		let ca_key = KeyPair::generate().expect("ca key");
		let issuer = Issuer::new(ca_params, ca_key);

		let now = time::OffsetDateTime::now_utc();
		let params = CertificateRevocationListParams {
			this_update: now,
			next_update: now + time::Duration::hours(24),
			crl_number: SerialNumber::from(1u64),
			issuing_distribution_point: None,
			revoked_certs: vec![RevokedCertParams {
				serial_number: SerialNumber::from(42u64),
				revocation_time: now,
				reason_code: Some(RevocationReason::KeyCompromise),
				invalidity_date: None,
			}],
			key_identifier_method: KeyIdMethod::Sha256,
		};
		let crl = params.signed_by(&issuer).expect("sign crl");
		crl.der().as_ref().to_vec()
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn snapshot_serves_same_arc_for_same_source() {
		let bytes = fixture_crl_bytes();
		let fetcher = Arc::new(StaticFetcher { bytes, count: AtomicUsize::new(0) });
		let cache = CrlCache::new(fetcher.clone());
		let src = CrlSourceId::Url("https://crl.example/fixture".into());
		cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)]).expect("load");
		let s1 = cache.snapshot(std::slice::from_ref(&src)).expect("snap");
		let s2 = cache.snapshot(std::slice::from_ref(&src)).expect("snap");
		assert_eq!(s1.len(), 1);
		assert!(Arc::ptr_eq(&s1[0], &s2[0]), "snapshot must clone same Arc");
		assert_eq!(fetcher.count.load(Ordering::SeqCst), 1, "no extra fetches");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn tolerate_unavailable_silently_drops_source() {
		let fetcher = Arc::new(AlwaysFailFetcher { count: AtomicUsize::new(0) });
		let cache = CrlCache::new(fetcher);
		let src = CrlSourceId::Url("https://crl.example/down".into());
		cache
			.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)])
			.expect("tolerate must not propagate");
		let snap = cache.snapshot(&[src]).expect("snapshot ok");
		assert!(snap.is_empty(), "tolerate + never-loaded => silently dropped");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn reject_unavailable_returns_err_at_link() {
		let fetcher = Arc::new(AlwaysFailFetcher { count: AtomicUsize::new(0) });
		let cache = CrlCache::new(fetcher);
		let src = CrlSourceId::Url("https://crl.example/down".into());
		let err =
			cache.ensure_loaded(&[(src, CrlFetchFailure::Reject)]).expect_err("reject must fail-closed");
		assert!(err.contains("fixture failure"), "{err}");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn reject_unavailable_returns_err_at_snapshot() {
		let fetcher = Arc::new(AlwaysFailFetcher { count: AtomicUsize::new(0) });
		let cache = CrlCache::new(fetcher);
		let src = CrlSourceId::Url("https://crl.example/down".into());
		// Tolerate at link time so ensure_loaded returns Ok, then ask
		// for a reject snapshot — same entry, harder policy. The
		// snapshot path independently checks reject + unavailable.
		cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)]).expect("tolerate at link");
		assert!(cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Reject)]).is_err());
		let snap_err = cache.snapshot(&[src]).expect_err("reject snapshot must fail-closed");
		assert!(snap_err.contains("unavailable"), "{snap_err}");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn next_update_parsed_from_fixture_crl() {
		let bytes = fixture_crl_bytes();
		let nu = parse_next_update(&bytes).expect("nextUpdate present");
		assert!(nu > time::OffsetDateTime::now_utc(), "fixture nextUpdate is in future");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn refresh_loop_updates_bytes_in_place() {
		let bytes = fixture_crl_bytes();
		let fetcher =
			Arc::new(FlippingFetcher { ok_bytes: bytes.clone(), succeed: AtomicBool::new(true) });
		let cache = CrlCache::new(fetcher.clone());
		let src = CrlSourceId::Url("https://crl.example/flipping".into());
		cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)]).expect("initial load");
		let first = cache.snapshot(std::slice::from_ref(&src)).expect("snap");
		assert_eq!(first.len(), 1);

		fetcher.succeed.store(false, Ordering::SeqCst);
		cache
			.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)])
			.expect("tolerate keeps last-known bytes");

		let after = cache.snapshot(&[src]).expect("snap");
		assert_eq!(after.len(), 1);
		assert!(Arc::ptr_eq(&first[0], &after[0]), "Arc identity preserved across failed refresh");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn snapshot_unknown_source_errors() {
		let fetcher = Arc::new(StaticFetcher { bytes: vec![], count: AtomicUsize::new(0) });
		let cache = CrlCache::new(fetcher);
		let src = CrlSourceId::Url("https://crl.example/never-loaded".into());
		assert!(cache.snapshot(&[src]).is_err());
	}

	#[test]
	fn dedupe_picks_strictest_policy() {
		let src = CrlSourceId::from_url("https://crl.example/x");
		let out = dedupe_crl_sources([
			(src.clone(), CrlFetchFailure::Tolerate),
			(src.clone(), CrlFetchFailure::Reject),
			(src.clone(), CrlFetchFailure::Tolerate),
		]);
		assert_eq!(out.len(), 1);
		assert!(matches!(out[0].1, CrlFetchFailure::Reject));
	}
}
