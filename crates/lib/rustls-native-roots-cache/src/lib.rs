//! Process-wide cache for the system trust store.
//!
//! [`rustls_native_certs::load_native_certs`] reaches into the OS
//! keychain (Security framework on macOS, NSS / OpenSSL stores on
//! Linux). On macOS the underlying `Sec*` APIs are not concurrency-safe
//! under load — multiple threads calling them in parallel can return
//! `errSecIO` (-36) on what would otherwise succeed. Production
//! daemons that build many distinct rustls `ClientConfig`s (one per
//! upstream-TLS fingerprint, e.g.) hit this whenever a reload
//! introduces a handful of new fingerprints concurrently.
//!
//! The fix is a process-wide cache: read the trust store **once per
//! process**, share the resulting [`rustls::RootCertStore`] behind
//! `Arc`. The first call's init barrier serialises the (single) load
//! attempt; every subsequent caller gets a cheap `Arc::clone`.
//!
//! In-process the cache is sufficient. Across processes (e.g. a test
//! runner that boots multiple binaries in parallel) each binary still
//! makes its own first call, and those simultaneous calls can lose to
//! keychain contention. The init path therefore retries on transient
//! failure with a small backoff before giving up — `errSecIO` is
//! documented by Apple as recoverable, and the happy path skips the
//! backoff entirely.
//!
//! Long-running daemons need to pick up CA-cert updates (an OS
//! security update revoking a root, an operator dropping a corporate
//! CA into the keychain) without restarting. [`refresh_native_roots`]
//! re-runs the load and atomically swaps the cached store on success;
//! on failure the previous value is preserved and a warning is
//! logged. The swap is lock-free on the read side so the upstream-TLS
//! hot path is unaffected.

use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;

/// Shared error type. Carries an operator-readable message; the
/// underlying `rustls_native_certs::Error` is not `Clone`, so we
/// stringify at first-failure time and re-yield the same string on
/// subsequent calls.
#[derive(Debug, Clone)]
pub struct NativeRootsError {
	pub message: String,
}

impl std::fmt::Display for NativeRootsError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.message)
	}
}

impl std::error::Error for NativeRootsError {}

/// Cached load outcome. Cloned cheaply behind `Arc` so `load_full`
/// hands callers an owned snapshot without a per-call deep copy.
type Cached = Arc<Result<Arc<rustls::RootCertStore>, NativeRootsError>>;

/// Lazy-initialised current trust-store snapshot. The first call
/// through [`native_roots`] populates this; subsequent calls,
/// including [`refresh_native_roots`], swap the inner value through
/// `ArcSwap` without invalidating the [`OnceLock`].
static NATIVE_ROOTS: OnceLock<ArcSwap<Result<Arc<rustls::RootCertStore>, NativeRootsError>>> =
	OnceLock::new();

fn snapshot() -> &'static ArcSwap<Result<Arc<rustls::RootCertStore>, NativeRootsError>> {
	NATIVE_ROOTS.get_or_init(|| ArcSwap::from(Arc::new(load_native_roots())))
}

/// Return the cached system trust store, loading it on first call.
///
/// Concurrent first calls are serialised by the [`OnceLock`] barrier,
/// so the OS keychain sees exactly one load attempt per process even
/// under reload pressure that builds many fingerprints in parallel.
/// Subsequent calls are lock-free: they read the current snapshot
/// through `ArcSwap` and clone the inner `Arc<RootCertStore>`.
///
/// # Errors
///
/// Surfaces the most recently observed load outcome. A failed first
/// load remains sticky until [`refresh_native_roots`] succeeds.
pub fn native_roots() -> Result<Arc<rustls::RootCertStore>, NativeRootsError> {
	let cached: Cached = snapshot().load_full();
	cached.as_ref().as_ref().map(Arc::clone).map_err(Clone::clone)
}

/// Eagerly trigger the first load. Useful when a daemon's boot path
/// wants to know the trust-store status before any TLS code runs —
/// idempotent; subsequent calls return the cached result without
/// re-touching the OS keychain.
///
/// # Errors
///
/// Same shape as [`native_roots`]: returns the cached error if the
/// load failed.
pub fn warm_native_roots() -> Result<Arc<rustls::RootCertStore>, NativeRootsError> {
	native_roots()
}

/// Re-read the OS trust store and atomically swap the cached
/// snapshot when the load succeeds.
///
/// Long-lived daemons call this on a periodic timer or in response
/// to an operator-triggered mgmt verb so OS-side CA updates land
/// without a process restart. On failure the previous snapshot is
/// preserved and a warning is logged — operators still see a working
/// trust store while the load error surfaces in the warn record.
///
/// # Errors
///
/// Returns the new load attempt's error verbatim. The cached value
/// is **not** replaced with the error in that case; subsequent
/// [`native_roots`] callers continue to see whichever outcome was
/// last cached (typically the prior successful store).
pub fn refresh_native_roots() -> Result<Arc<rustls::RootCertStore>, NativeRootsError> {
	let outcome = load_native_roots();
	match &outcome {
		Ok(store) => {
			snapshot().store(Arc::new(Ok(Arc::clone(store))));
			Ok(Arc::clone(store))
		}
		Err(e) => {
			tracing::warn!(
				error = %e,
				"native trust store refresh failed; keeping previous snapshot",
			);
			Err(e.clone())
		}
	}
}

/// Maximum number of attempts at loading the OS trust store.
///
/// macOS Security framework returns `errSecIO` (-36) on transient
/// I/O failure when the keychain APIs see concurrent callers (e.g. a
/// test runner spawns dozens of test binaries that each boot their
/// own process and hit `load_native_certs` simultaneously). Apple's
/// own framework documents the error as recoverable. The happy path
/// completes in attempt 1 with zero sleeps; only an observed failure
/// pays the backoff. The [`OnceLock`] cache means we pay this cost
/// at most once per process lifetime.
const LOAD_RETRIES: usize = 3;
const LOAD_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

fn load_native_roots() -> Result<Arc<rustls::RootCertStore>, NativeRootsError> {
	let started = std::time::Instant::now();
	let mut last_err: Option<NativeRootsError> = None;
	for attempt in 0..LOAD_RETRIES {
		match try_load_native_roots() {
			Ok(store) => {
				tracing::info!(
					anchors = store.len(),
					elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
					attempts = attempt + 1,
					"native trust store loaded",
				);
				return Ok(store);
			}
			Err(e) => {
				last_err = Some(e);
				if attempt + 1 < LOAD_RETRIES {
					std::thread::sleep(LOAD_RETRY_BACKOFF);
				}
			}
		}
	}
	let err = last_err.expect("at least one attempt always populates last_err on the failure path");
	tracing::error!(
		error = %err,
		elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
		attempts = LOAD_RETRIES,
		"native trust store load failed",
	);
	Err(err)
}

fn try_load_native_roots() -> Result<Arc<rustls::RootCertStore>, NativeRootsError> {
	let native = rustls_native_certs::load_native_certs();
	if !native.errors.is_empty() {
		return Err(NativeRootsError { message: format!("load native certs: {:?}", native.errors) });
	}
	let mut store = rustls::RootCertStore::empty();
	for cert in native.certs {
		store.add(cert).map_err(|e| NativeRootsError { message: format!("add native cert: {e}") })?;
	}
	Ok(Arc::new(store))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn native_roots_returns_same_arc_across_calls() {
		// Single-process invariant: the OnceLock serves the same
		// underlying RootCertStore on every call. The keychain (or
		// equivalent OS trust store) is only touched during the very
		// first call across the entire test binary's lifetime.
		let a = native_roots().expect("trust store loads in test env");
		let b = native_roots().expect("cached call");
		assert!(Arc::ptr_eq(&a, &b), "subsequent calls must hand out the same Arc");
		assert!(!a.is_empty(), "system trust store should have at least one anchor");
	}

	#[test]
	fn warm_native_roots_returns_same_result_as_lazy_call() {
		let warmed = warm_native_roots().expect("warm");
		let lazy = native_roots().expect("lazy");
		assert!(Arc::ptr_eq(&warmed, &lazy));
	}

	#[test]
	fn refresh_native_roots_swaps_to_a_fresh_arc() {
		// A successful refresh must publish a *new* Arc so callers
		// re-reading `native_roots` see the new value (even when the
		// keychain contents happen to be identical). Pointer
		// inequality is the proxy.
		let before = native_roots().expect("first load");
		let refreshed = refresh_native_roots().expect("refresh");
		assert!(!Arc::ptr_eq(&before, &refreshed), "refresh swaps Arc identity");
		let after = native_roots().expect("post-refresh");
		assert!(Arc::ptr_eq(&refreshed, &after), "subsequent reads see refreshed snapshot");
	}
}
