//! See `README.md` for the operator-facing pitch.
//!
//! `InFlightSet` wraps `tokio::task::JoinSet<()>` behind a
//! `std::sync::Mutex` so accept loops can `spawn` from sync context
//! and a single shutdown driver can `drain` cooperatively (with or
//! without `abort_all`) without ever holding the lock across an
//! `.await`. The "take the JoinSet out under a brief critical section,
//! then drive `join_next` off-lock" pattern is the load-bearing
//! invariant — every method respects it.

use std::future::Future;
use std::sync::Mutex;

use tokio::task::JoinSet;

/// Shareable supervisor for one-shot tasks. Construct once per
/// supervised set (one per listener, typically), wrap in `Arc`, hand
/// to the accept loop and the shutdown driver.
///
/// `T = ()` is the dominant case (per-connection tasks return
/// nothing); leave it as default unless the caller needs to inspect
/// task return values.
pub struct InFlightSet<T = ()> {
	inner: Mutex<JoinSet<T>>,
}

impl<T> Default for InFlightSet<T> {
	fn default() -> Self {
		Self::new()
	}
}

impl<T> InFlightSet<T> {
	/// Build an empty set.
	#[must_use]
	pub fn new() -> Self {
		Self { inner: Mutex::new(JoinSet::new()) }
	}

	/// Number of tasks currently tracked. Some entries may already
	/// have finished — `JoinSet` reports finished-but-not-joined
	/// tasks as still present until something calls `join_next`.
	///
	/// # Panics
	/// Panics if the internal mutex has been poisoned by a prior
	/// panic. The only contained operations are `JoinSet::spawn`,
	/// `JoinSet::len`, `JoinSet::is_empty`, and `mem::replace`, none
	/// of which can panic in practice — so a poisoned lock indicates
	/// a fatal invariant break upstream.
	#[must_use]
	pub fn len(&self) -> usize {
		self.inner.lock().expect("in_flight mutex poisoned").len()
	}

	/// True when no tasks are currently tracked.
	///
	/// # Panics
	/// Panics if the internal mutex has been poisoned. See
	/// [`Self::len`] for the rationale.
	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.inner.lock().expect("in_flight mutex poisoned").is_empty()
	}
}

impl<T: Send + 'static> InFlightSet<T> {
	/// Spawn `future` onto the supervised set. Synchronous on the
	/// caller's side — the only locking touch is the brief sync
	/// critical section around `JoinSet::spawn`.
	///
	/// # Panics
	/// Panics if the internal mutex has been poisoned. See
	/// [`Self::len`] for the rationale.
	pub fn spawn<F>(&self, future: F)
	where
		F: Future<Output = T> + Send + 'static,
	{
		self.inner.lock().expect("in_flight mutex poisoned").spawn(future);
	}

	/// Drain every tracked task to completion. Holds the sync mutex
	/// only for the `mem::replace` that takes the set out; the
	/// `join_next` loop runs off-lock so callers that hold an
	/// `Arc<InFlightSet>` from elsewhere can still spawn into a fresh
	/// (empty) `JoinSet` while the drain proceeds. Returns once every
	/// taken task has been joined.
	///
	/// # Panics
	/// Panics if the internal mutex has been poisoned. See
	/// [`Self::len`] for the rationale.
	pub async fn drain(&self) {
		let mut taken = self.take_inner();
		while taken.join_next().await.is_some() {}
	}

	/// Same as [`Self::drain`] but calls `JoinSet::abort_all` on the
	/// taken set before joining. Used as the second stage of a
	/// cooperative drain (soft-drain timed out → fire abort).
	///
	/// # Panics
	/// Panics if the internal mutex has been poisoned. See
	/// [`Self::len`] for the rationale.
	pub async fn drain_with_abort(&self) {
		let mut taken = self.take_inner();
		taken.abort_all();
		while taken.join_next().await.is_some() {}
	}

	fn take_inner(&self) -> JoinSet<T> {
		let mut g = self.inner.lock().expect("in_flight mutex poisoned");
		std::mem::replace(&mut *g, JoinSet::new())
	}
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;
	use std::sync::atomic::{AtomicUsize, Ordering};
	use std::time::Duration;

	use super::*;

	#[tokio::test]
	async fn spawn_then_drain_runs_each_task_to_completion() {
		let set: Arc<InFlightSet> = Arc::new(InFlightSet::new());
		let counter = Arc::new(AtomicUsize::new(0));
		for _ in 0..32 {
			let c = Arc::clone(&counter);
			set.spawn(async move {
				c.fetch_add(1, Ordering::Relaxed);
			});
		}
		set.drain().await;
		assert_eq!(counter.load(Ordering::Relaxed), 32);
		assert!(set.is_empty());
	}

	#[tokio::test]
	async fn drain_with_abort_cancels_pending_sleeps() {
		let set: Arc<InFlightSet> = Arc::new(InFlightSet::new());
		let completed = Arc::new(AtomicUsize::new(0));
		for _ in 0..8 {
			let c = Arc::clone(&completed);
			set.spawn(async move {
				tokio::time::sleep(Duration::from_secs(30)).await;
				c.fetch_add(1, Ordering::Relaxed);
			});
		}
		// Force-cancel everything before any task hits its sleep
		// deadline — none should run the `fetch_add`.
		set.drain_with_abort().await;
		assert_eq!(completed.load(Ordering::Relaxed), 0);
		assert!(set.is_empty());
	}

	#[tokio::test]
	async fn spawn_into_arc_clones_observes_same_set() {
		let set: Arc<InFlightSet> = Arc::new(InFlightSet::new());
		let counter = Arc::new(AtomicUsize::new(0));
		let b = Arc::clone(&set);
		let c = Arc::clone(&counter);
		// Spawn through one Arc handle…
		set.spawn(async move {
			c.fetch_add(1, Ordering::Relaxed);
		});
		// …drain through another.
		b.drain().await;
		assert_eq!(counter.load(Ordering::Relaxed), 1);
	}
}
