//! Daemon-wide concurrency cap on simultaneously running CGI
//! children. Single global semaphore + atomic counters; the runtime
//! path enters via `cgi_permits()` / `cgi_permit_counters()` and the
//! mgmt-verb layer snapshots via [`pool_stats`].
//!
//! Per `spec/crates/engine.md` § _Concurrency cap_: when the cap is
//! reached, new requests fast-reject with 503 — no queueing — and the
//! `failures` counter increments. Successful permit acquisitions bump
//! `total_spawns`, which surfaces on the wire as `total_allocations`.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};

use tokio::sync::Semaphore;

/// Snapshot of the CGI concurrency cap. Read-only: returns `None`
/// until the semaphore is lazily initialised on the first CGI request.
///
/// The mgmt-verb path must not trigger first-init — operators reading
/// `get_pools` before any CGI traffic should see the absent state, not
/// implicitly bake `VANE_CGI_MAX_CONCURRENT` into a process-wide
/// constant on a cold daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CgiPoolStats {
	pub cap: usize,
	pub available: usize,
	pub in_use: usize,
	/// Cumulative successful permit acquisitions — translated to
	/// `total_allocations` on the wire shape.
	pub total_allocations: u64,
	/// Cumulative cap-rejected acquisitions.
	pub failures: u64,
}

/// Daemon-wide cap on simultaneously running CGI children. `spec/crates/engine.md`
/// § _Concurrency cap_: when reached, new requests fast-reject with 503;
/// no queueing.
///
/// The semaphore is built once per process from
/// `VANE_CGI_MAX_CONCURRENT` (default 100). The `OnceLock` initializer
/// runs lazily on the first CGI request — daemon init does not need
/// to poke the slot.
///
/// `cap` is captured alongside the [`Semaphore`] so `pool_stats()` can
/// report `(cap, available)` consistently — `tokio::sync::Semaphore`
/// itself does not expose its initial permit count, and re-reading
/// `VANE_CGI_MAX_CONCURRENT` would race with operator-side env churn.
struct CgiPermitState {
	semaphore: Arc<Semaphore>,
	cap: usize,
	/// Cumulative successful permit acquisitions — i.e. CGI fetches that
	/// crossed the cap gate and proceeded to fork/exec.
	total_spawns: Arc<AtomicU64>,
	/// Cumulative `try_acquire_owned` failures — fast-rejects under the
	/// concurrency cap (`spec/crates/engine.md` § _Concurrency cap_).
	failures: Arc<AtomicU64>,
}

static CGI_PERMITS: OnceLock<CgiPermitState> = OnceLock::new();

const DEFAULT_MAX_CONCURRENT: usize = 100;

pub(super) fn cgi_permits() -> Arc<Semaphore> {
	Arc::clone(
		&CGI_PERMITS
			.get_or_init(|| {
				let cap = std::env::var("VANE_CGI_MAX_CONCURRENT")
					.ok()
					.and_then(|s| s.parse::<usize>().ok())
					.filter(|n| *n > 0)
					.unwrap_or(DEFAULT_MAX_CONCURRENT);
				CgiPermitState {
					semaphore: Arc::new(Semaphore::new(cap)),
					cap,
					total_spawns: Arc::new(AtomicU64::new(0)),
					failures: Arc::new(AtomicU64::new(0)),
				}
			})
			.semaphore,
	)
}

/// Counter handles tied to the lazily-initialised permit state. Returns
/// `None` when the state has not yet been touched (no CGI traffic yet).
pub(super) fn cgi_permit_counters() -> Option<(Arc<AtomicU64>, Arc<AtomicU64>)> {
	let state = CGI_PERMITS.get()?;
	Some((Arc::clone(&state.total_spawns), Arc::clone(&state.failures)))
}

#[must_use]
pub fn pool_stats() -> Option<CgiPoolStats> {
	let state = CGI_PERMITS.get()?;
	let available = state.semaphore.available_permits();
	let in_use = state.cap.saturating_sub(available);
	Some(CgiPoolStats {
		cap: state.cap,
		available,
		in_use,
		total_allocations: state.total_spawns.load(std::sync::atomic::Ordering::Relaxed),
		failures: state.failures.load(std::sync::atomic::Ordering::Relaxed),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pool_stats_reports_state_after_first_init() {
		// Drive the lazy init exactly once via the same code path that
		// CgiFetch::fetch uses. Once the semaphore is live, pool_stats
		// must report a fully-available pool (no permits held).
		//
		// Cannot assert the pre-init `None` shape here because other
		// unit tests in the crate's test binary may have already fired
		// the OnceLock; the dispatcher / e2e tests cover that arm.
		let _ = cgi_permits();
		let stats = pool_stats().expect("semaphore initialised");
		assert!(stats.cap > 0);
		assert_eq!(stats.available, stats.cap, "no in-flight CGI children in this test binary");
		assert_eq!(stats.in_use, 0);
	}
}
