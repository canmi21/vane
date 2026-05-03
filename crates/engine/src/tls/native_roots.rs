//! Process-wide cache for the system trust store.
//!
//! [`rustls_native_certs::load_native_certs`] reaches into the OS
//! keychain (Security framework on macOS, NSS / OpenSSL stores on
//! Linux). On macOS the underlying `Sec*` APIs are not concurrency-safe
//! under load — multiple threads calling them in parallel can return
//! `errSecIO` (-36) on what would otherwise succeed. Production daemons
//! that build many distinct rustls `ClientConfig`s (one per
//! upstream-TLS fingerprint) hit this whenever a reload introduces a
//! handful of new fingerprints concurrently.
//!
//! The fix is an architectural staple: read the trust store **once per
//! process**, share the resulting [`rustls::RootCertStore`] behind
//! `Arc`. The `OnceLock` initializer barrier serialises the (single)
//! load attempt; every subsequent caller gets a cheap `Arc::clone`.
//!
//! In-process the `OnceLock` is sufficient. Across processes (e.g. a
//! nextest workspace boots multiple test binaries in parallel) each
//! binary still makes its own first call, and those simultaneous
//! calls can lose to keychain contention. The init path therefore
//! retries on transient failure with a small backoff before giving
//! up — `errSecIO` is documented by Apple as recoverable, and the
//! happy path skips the backoff entirely.
//!
//! Failure semantics: after the bounded retries are exhausted the
//! outcome is sticky. The cached error re-yields on every subsequent
//! call so the operator sees consistent behaviour and can restart
//! the daemon to attempt a fresh load. This matches the spec's
//! "explicit failure modes" posture and avoids per-request retry
//! storms against an OS API that's already telling us it's unhappy.

use std::sync::{Arc, OnceLock};

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

static NATIVE_ROOTS: OnceLock<Result<Arc<rustls::RootCertStore>, NativeRootsError>> =
	OnceLock::new();

/// Return the cached system trust store, loading it on first call.
///
/// Concurrent first calls are serialised by the `OnceLock` barrier,
/// so the OS keychain sees exactly one load attempt per process even
/// under reload pressure that builds many fingerprints in parallel.
///
/// # Errors
///
/// Surfaces the load attempt's error (sticky for the lifetime of the
/// process). Restart the daemon to retry.
pub fn native_roots() -> Result<Arc<rustls::RootCertStore>, NativeRootsError> {
	NATIVE_ROOTS.get_or_init(load_native_roots).as_ref().map(Arc::clone).map_err(Clone::clone)
}

/// Eagerly trigger the first load. Daemon boot calls this so the
/// trust store's status is known before the first reload races to
/// build factories. Idempotent — subsequent calls return the cached
/// result without re-touching the OS keychain.
///
/// # Errors
///
/// Same shape as [`native_roots`]: returns the cached error if the
/// load failed.
pub fn warm_native_roots() -> Result<Arc<rustls::RootCertStore>, NativeRootsError> {
	native_roots()
}

/// Maximum number of attempts at loading the OS trust store.
///
/// macOS Security framework returns `errSecIO` (-36) on transient
/// I/O failure when the keychain APIs see concurrent callers (e.g. a
/// nextest workspace run spawns dozens of test binaries that each
/// boot their own daemon and hit `load_native_certs` simultaneously).
/// Apple's own framework documents the error as recoverable. The
/// happy path completes in attempt 1 with zero sleeps; only an
/// observed failure pays the backoff. The `OnceLock` cache means we
/// pay this cost at most once per process lifetime.
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
		"native trust store load failed; HTTPS upstream rules without insecure_skip_verify will fail at use",
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
}
