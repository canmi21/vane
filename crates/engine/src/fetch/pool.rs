//! Centralised tunables for the per-upstream connection pool, plus
//! the cross-upstream `max_concurrent_per_host` enforcement that
//! `hyper-util`'s legacy [`Client`](hyper_util::client::legacy::Client)
//! does not expose natively.
//!
//! Numbers come from [`spec/crates/engine.md` § _Exhaustion defaults
//! (per upstream)_](../../../../spec/crates/engine.md#exhaustion-defaults-per-upstream)
//! and are reflected in the table there. Edits to the values in this
//! module must be paired with an edit to the spec.
//!
//! ## Why a separate module
//!
//! - **Auditability** — operators reading the spec table can `grep`
//!   for `MAX_IDLE_PER_HOST` and land on a single source of truth.
//! - **Sharing across fetch flavours** — the H1, H2 and H3 fetch
//!   paths all enforce the same per-authority concurrency limit, so
//!   the `AuthorityLimiter` lives once, not once per fetch type.
//! - **Decoupling from `http_proxy.rs`** — the pool tunables predate
//!   any concrete fetch implementation; keeping them outside
//!   `http_proxy.rs` lets new fetch types (CGI proxies, WS proxies)
//!   adopt the same SLA without a tangled import graph.
//!
//! ## What this module does NOT tune
//!
//! Hyper-util exposes more knobs (`pool_idle_timeout` per scheme,
//! H2-specific window sizes, etc.); only the four spec-documented
//! values plus the H2 `Rapid Reset` mitigation are surfaced here so
//! the spec table stays the canonical configuration matrix.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use dashmap::DashMap;
use http::uri::Authority;
use tokio::sync::Semaphore;
use vane_core::{Error, UpstreamReason};

/// Cap on idle (kept-alive) connections per upstream `Authority`.
///
/// Bounds the steady-state file-descriptor and memory footprint when
/// many transient request bursts settle into the pool. See
/// [`spec/crates/engine.md` § _Exhaustion defaults (per upstream)_].
pub(crate) const MAX_IDLE_PER_HOST: usize = 32;

/// Cap on **concurrent in-flight** requests per upstream `Authority`.
///
/// Hyper-util's [`Client`](hyper_util::client::legacy::Client) does
/// not enforce this natively, so [`AuthorityLimiter`] supplies a
/// per-authority [`Semaphore`] gating fetch entry. See
/// [`spec/crates/engine.md` § _Exhaustion defaults (per upstream)_].
pub(crate) const MAX_CONCURRENT_PER_HOST: usize = 100;

/// Bound on establishing a new upstream TCP connection (handshake
/// included for the underlying socket; TLS handshake runs after this
/// timeout has been satisfied at the TCP layer).
///
/// Threaded into the inner
/// [`HttpConnector`](hyper_util::client::legacy::connect::HttpConnector)
/// via `set_connect_timeout`. Also acts as the deadline for
/// [`AuthorityLimiter::acquire`] when the per-authority semaphore is
/// saturated — operators chose the same number on purpose: a fetch
/// that cannot enter the limiter within `CONNECT_TIMEOUT` would have
/// missed its `connect_timeout` deadline anyway.
///
/// See [`spec/crates/engine.md` § _Exhaustion defaults (per upstream)_].
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Idle close threshold. Hyper-util needs an explicit
/// [`TokioTimer`](hyper_util::rt::TokioTimer) plumbed through the
/// builder for this to actually fire — `pool_idle_timeout` alone is
/// a no-op without a timer source. See
/// [`spec/crates/engine.md` § _Exhaustion defaults (per upstream)_].
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// H2-specific cap on the number of locally-tracked
/// `RST_STREAM`-pending streams, mitigating CVE-2023-44487 ("HTTP/2
/// Rapid Reset"). Hyper's default of 1024 is far too generous for a
/// proxy that fans out to many upstreams; align with
/// [`MAX_IDLE_PER_HOST`] so a single misbehaving upstream cannot
/// burn arbitrary memory under reset spam.
pub(crate) const H2_MAX_CONCURRENT_RESET_STREAMS: usize = 32;

/// Per-authority concurrency gate. Backs the `max_concurrent_per_host`
/// promise that `hyper-util` does not enforce natively.
///
/// One [`Semaphore`] per upstream `Authority`. Lookups are
/// short-lived contended-only-on-insert — the [`DashMap`] handles
/// the read-mostly fast path and only resorts to a write-lock when a
/// brand-new authority is seen.
pub(crate) struct AuthorityLimiter {
	slots: DashMap<Authority, Arc<Semaphore>>,
	per_host: usize,
}

impl AuthorityLimiter {
	#[must_use]
	pub(crate) fn with_per_host(per_host: usize) -> Self {
		// `DashMap::new` is not `const`, so the constructor cannot be
		// either; callers should park instances in a `OnceLock` to
		// keep the limiter process-wide.
		Self { slots: DashMap::new(), per_host }
	}

	fn slot(&self, authority: &Authority) -> Arc<Semaphore> {
		if let Some(s) = self.slots.get(authority) {
			return Arc::clone(s.value());
		}
		Arc::clone(
			self
				.slots
				.entry(authority.clone())
				.or_insert_with(|| Arc::new(Semaphore::new(self.per_host)))
				.value(),
		)
	}

	/// Acquire one permit for `authority`, bounding the wait by
	/// [`CONNECT_TIMEOUT`]. Returns a permit guard that releases on
	/// drop; the caller must hold it for the duration of the fetch.
	///
	/// # Errors
	/// Returns [`Error::upstream`] with
	/// [`UpstreamReason::Unreachable`] when the wait exceeds the
	/// deadline — surfaces to the client as `503` per
	/// [`spec/crates/engine.md` § _Error classification_].
	pub(crate) async fn acquire(&self, authority: &Authority) -> Result<LimiterPermit, Error> {
		self.acquire_with_timeout(authority, CONNECT_TIMEOUT).await
	}

	/// Same contract as [`Self::acquire`] but with a caller-supplied
	/// deadline. Exists so unit tests can exercise the saturation
	/// path without burning [`CONNECT_TIMEOUT`] of wall time per
	/// assertion; production callers always go through
	/// [`Self::acquire`].
	pub(crate) async fn acquire_with_timeout(
		&self,
		authority: &Authority,
		timeout: Duration,
	) -> Result<LimiterPermit, Error> {
		let sem = self.slot(authority);
		match tokio::time::timeout(timeout, sem.acquire_owned()).await {
			Ok(Ok(permit)) => Ok(LimiterPermit(Some(permit))),
			Ok(Err(_closed)) => {
				// Semaphore::close is never called in this module; the
				// only way this branch fires is a programming error, so
				// surface it as an unreachable upstream rather than
				// pretending the request can proceed.
				Err(Error::upstream(UpstreamReason::Unreachable).with_ctx("authority limiter closed"))
			}
			Err(_elapsed) => Err(Error::upstream(UpstreamReason::Unreachable).with_ctx(format!(
				"max_concurrent_per_host ({}) saturated for {}",
				self.per_host, authority,
			))),
		}
	}
}

/// RAII guard returned by [`AuthorityLimiter::acquire`]. Permit is
/// released on drop.
#[derive(Debug)]
pub(crate) struct LimiterPermit(#[allow(dead_code)] Option<tokio::sync::OwnedSemaphorePermit>);

/// Process-wide limiter shared across all `HttpProxyFetch` instances.
/// The semaphore population is bounded by the number of distinct
/// authorities ever observed in a `vaned` lifetime; tens of thousands
/// of entries is fine — each `Semaphore` is ~32 bytes plus the
/// `Arc` header.
pub(crate) fn limiter() -> &'static AuthorityLimiter {
	static LIMITER: OnceLock<AuthorityLimiter> = OnceLock::new();
	LIMITER.get_or_init(|| AuthorityLimiter::with_per_host(MAX_CONCURRENT_PER_HOST))
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicUsize, Ordering};

	use tokio::time::Duration as TokioDuration;

	use super::*;

	fn authority(s: &str) -> Authority {
		s.parse().expect("valid authority literal")
	}

	#[tokio::test]
	async fn acquire_releases_on_permit_drop() {
		let lim = AuthorityLimiter::with_per_host(1);
		let a = authority("api.example.com:443");
		{
			let _p = lim.acquire(&a).await.expect("first permit");
			// Second `acquire` would block; verify the first does not
			// leak by dropping the guard explicitly and re-acquiring.
		}
		let _again = lim.acquire(&a).await.expect("re-acquire after drop");
	}

	#[tokio::test]
	async fn distinct_authorities_do_not_share_permits() {
		let lim = AuthorityLimiter::with_per_host(1);
		let a = authority("api.example.com:443");
		let b = authority("backup.example.com:443");
		let _pa = lim.acquire(&a).await.expect("permit a");
		// `b` is independent — must not wait.
		let _pb = tokio::time::timeout(TokioDuration::from_millis(50), lim.acquire(&b))
			.await
			.expect("acquire on distinct authority must not block past spec timeout")
			.expect("permit b");
	}

	#[tokio::test]
	async fn saturation_returns_unreachable_with_naming_context() {
		let lim = AuthorityLimiter::with_per_host(1);
		let a = authority("api.example.com:443");
		let _hold = lim.acquire(&a).await.expect("hold permit");
		// Use the test-only timeout knob so the assertion runs in
		// milliseconds rather than the spec's 5 s production budget.
		let err = lim
			.acquire_with_timeout(&a, TokioDuration::from_millis(50))
			.await
			.expect_err("saturated acquire must error");
		let msg = err.to_string();
		assert!(
			msg.contains("max_concurrent_per_host") && msg.contains("api.example.com:443"),
			"error context names limit and authority: {msg}",
		);
	}

	#[tokio::test]
	async fn concurrent_acquires_serialize_under_per_host_cap() {
		// Spawn `per_host + N` waiters against the same authority and
		// confirm at most `per_host` are in flight at once.
		let lim = Arc::new(AuthorityLimiter::with_per_host(3));
		let in_flight = Arc::new(AtomicUsize::new(0));
		let peak = Arc::new(AtomicUsize::new(0));
		let mut tasks = Vec::new();
		for _ in 0..10 {
			let lim = Arc::clone(&lim);
			let inf = Arc::clone(&in_flight);
			let peak = Arc::clone(&peak);
			tasks.push(tokio::spawn(async move {
				let _p = lim.acquire(&authority("h:1")).await.expect("permit");
				let n = inf.fetch_add(1, Ordering::SeqCst) + 1;
				peak.fetch_max(n, Ordering::SeqCst);
				tokio::time::sleep(TokioDuration::from_millis(5)).await;
				inf.fetch_sub(1, Ordering::SeqCst);
			}));
		}
		for t in tasks {
			t.await.expect("waiter ok");
		}
		assert!(peak.load(Ordering::SeqCst) <= 3, "concurrent in-flight must not exceed per_host cap");
	}
}
