//! Two-phase wrapper around [`notify_debouncer_full`] that closes the
//! startup gap between server-bind and fs-watcher-subscribe.
//!
//! See the crate-level README for the full motivation. The short
//! version: on macOS `FSEvents`, an event that lands between your
//! listener bind and your watcher subscription is silently dropped.
//! Subscribing **before** the bind, queueing events into an unbounded
//! channel during the gap, and draining them after the rest of the
//! daemon is up fixes the race.
//!
//! Pipeline:
//!
//! 1. [`arm`] (or [`arm_with`]) returns a [`Subscription`]. The
//!    underlying `notify` watcher is already running; events are
//!    flowing into an internal mpsc.
//! 2. Bind listeners, finish boot, etc.
//! 3. Spawn a tokio task that loops over [`Subscription::recv`] —
//!    each `Some(())` is a debounced batch that the filter decided
//!    is interesting.
//! 4. Drop the `Subscription` (or break out of the loop) to stop
//!    watching.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::RecursiveMode;
use notify::event::{EventKind, ModifyKind};
use notify_debouncer_full::{
	DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};
use tokio::sync::mpsc;

// Re-export the upstream crates so callers don't have to add them as
// direct dependencies just to name the event types in their filter.
pub use notify;
pub use notify_debouncer_full;

/// Concrete debouncer type — the platform-recommended watcher backend
/// (`FSEvents` on macOS, inotify on Linux) plus the recommended cache.
type WatchHandle = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

/// Default debounce window. Matches what most config-watching daemons
/// pick; long enough to coalesce an editor's "save → atime → close"
/// sequence into a single batch, short enough to feel live.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);

/// Active fs-watcher subscription. Drop the value to stop watching.
///
/// Held by the caller across the listener-bind window so events
/// landing in the gap are queued, not lost.
pub struct Subscription {
	// `_debouncer` is held only for its `Drop` semantics — dropping
	// the value stops the underlying `notify::RecommendedWatcher`'s
	// background thread.
	_debouncer: WatchHandle,
	rx: mpsc::UnboundedReceiver<()>,
	watch_root: PathBuf,
}

impl Subscription {
	/// Path the watcher was armed against, post-canonicalisation. Note
	/// this may differ from what was passed to [`arm`] — e.g. on macOS
	/// `FSEvents` reports paths under `/private/var/folders/…` while the
	/// caller may have handed in the symlinked `/var/folders/…` form.
	#[must_use]
	pub fn watch_root(&self) -> &Path {
		&self.watch_root
	}

	/// Await the next reload-worthy debounced batch. Returns `None`
	/// once the underlying channel closes (this can only happen if
	/// the watcher backend itself disappears; in normal operation
	/// dropping the `Subscription` is what ends the loop).
	pub async fn recv(&mut self) -> Option<()> {
		self.rx.recv().await
	}
}

/// Arm a recursive fs-watcher with reasonable defaults — 250 ms
/// debounce window, the [`is_reloadable_batch`] predicate.
///
/// # Errors
///
/// Propagates `notify::Error` from the underlying watcher backend
/// (typically permission-denied on the directory).
pub fn arm(path: impl Into<PathBuf>) -> Result<Subscription, notify::Error> {
	arm_with(path, DEFAULT_DEBOUNCE, is_reloadable_batch)
}

/// Arm a recursive fs-watcher with a custom debounce window and a
/// custom batch-filter predicate. The predicate gets the watch root
/// (post-canonicalisation) so it can perform path-prefix checks.
///
/// # Errors
///
/// Propagates `notify::Error` from the underlying watcher backend.
pub fn arm_with<F>(
	path: impl Into<PathBuf>,
	debounce: Duration,
	filter: F,
) -> Result<Subscription, notify::Error>
where
	F: Fn(&[DebouncedEvent], &Path) -> bool + Send + 'static,
{
	let path = path.into();
	let (tx, rx) = mpsc::unbounded_channel::<()>();

	// Canonicalise the watch root so `starts_with` in the predicate
	// matches what `notify` reports. macOS FSEvents returns paths
	// under `/private/var/folders/...` while a `tempfile::tempdir()`
	// may give the symlinked `/var/folders/...` form; without this
	// the prefix check rejects every legitimate event.
	let watch_root = path.canonicalize().unwrap_or_else(|_| path.clone());
	let cb_root = watch_root.clone();

	let mut debouncer = new_debouncer(debounce, None, move |res: DebounceEventResult| {
		let Ok(events) = res else { return };
		if filter(&events, &cb_root) {
			// Coalesce: a single () per debounce window. Receiver
			// is unbounded so send is sync-ok.
			let _ = tx.send(());
		}
	})?;
	debouncer.watch(&path, RecursiveMode::Recursive)?;

	Ok(Subscription { _debouncer: debouncer, rx, watch_root })
}

/// The default batch-filter predicate. A debounced batch is reload-
/// worthy when at least one of its events:
///
/// - is one of `Create`, `Modify(Data)`, `Modify(Name)`, `Remove`
///   (see [`is_reloadable_kind`]); and
/// - touches at least one path under `watch_root`.
///
/// Events filtered out:
///
/// - `Access(_)` — atime / open / close, never affects content.
/// - `Modify(Metadata(_))` — chmod / chown / utime, no content change.
/// - `Modify(Other | Any)` and the top-level `Other` / `Any` — kept
///   conservative because backends differ. Idempotency in the caller's
///   downstream reload step is the safety net.
#[must_use]
pub fn is_reloadable_batch(events: &[DebouncedEvent], watch_root: &Path) -> bool {
	events.iter().any(|debounced| {
		is_reloadable_kind(debounced.event.kind)
			&& debounced.event.paths.iter().any(|p| p.starts_with(watch_root))
	})
}

/// Whether a single [`EventKind`] is one of the reload-worthy
/// categories used by [`is_reloadable_batch`].
#[must_use]
pub fn is_reloadable_kind(kind: EventKind) -> bool {
	match kind {
		EventKind::Create(_)
		| EventKind::Remove(_)
		| EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_)) => true,
		EventKind::Access(_)
		| EventKind::Modify(ModifyKind::Metadata(_) | ModifyKind::Other | ModifyKind::Any)
		| EventKind::Any
		| EventKind::Other => false,
	}
}

#[cfg(test)]
mod tests {
	use std::time::Instant;

	use notify::event::{
		AccessKind, CreateKind, Event as NotifyEvent, ModifyKind, RemoveKind, RenameMode,
	};
	use notify_debouncer_full::DebouncedEvent as DEvent;

	use super::*;

	fn ev_under(root: &Path, kind: EventKind) -> DEvent {
		let event = NotifyEvent::new(kind).add_path(root.join("rules").join("foo.json"));
		DEvent::new(event, Instant::now())
	}

	fn ev_outside(kind: EventKind) -> DEvent {
		let event = NotifyEvent::new(kind).add_path(PathBuf::from("/elsewhere/file.json"));
		DEvent::new(event, Instant::now())
	}

	#[test]
	fn filter_accepts_create_modify_data_rename_remove() {
		let root = PathBuf::from("/tmp/notify-twophase-fixture");
		for kind in [
			EventKind::Create(CreateKind::File),
			EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)),
			EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
			EventKind::Remove(RemoveKind::File),
		] {
			let batch = vec![ev_under(&root, kind)];
			assert!(is_reloadable_batch(&batch, &root), "reload-worthy kind rejected: {kind:?}");
		}
	}

	#[test]
	fn filter_rejects_metadata_access_and_unknown() {
		let root = PathBuf::from("/tmp/notify-twophase-fixture");
		for kind in [
			EventKind::Access(AccessKind::Read),
			EventKind::Modify(ModifyKind::Metadata(notify::event::MetadataKind::Permissions)),
			EventKind::Modify(ModifyKind::Other),
			EventKind::Modify(ModifyKind::Any),
			EventKind::Other,
			EventKind::Any,
		] {
			let batch = vec![ev_under(&root, kind)];
			assert!(!is_reloadable_batch(&batch, &root), "non-reload kind accepted: {kind:?}");
		}
	}

	#[test]
	fn filter_rejects_event_outside_watch_root() {
		let root = PathBuf::from("/tmp/notify-twophase-fixture");
		let batch = vec![ev_outside(EventKind::Create(CreateKind::File))];
		assert!(!is_reloadable_batch(&batch, &root));
	}

	#[test]
	fn filter_accepts_when_at_least_one_event_qualifies() {
		// Mixed batch: a metadata event alone wouldn't trigger;
		// pairing it with a create event under the watch root must.
		let root = PathBuf::from("/tmp/notify-twophase-fixture");
		let batch = vec![
			ev_under(
				&root,
				EventKind::Modify(ModifyKind::Metadata(notify::event::MetadataKind::Permissions)),
			),
			ev_under(&root, EventKind::Create(CreateKind::File)),
		];
		assert!(is_reloadable_batch(&batch, &root));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn arm_recv_picks_up_a_real_file_create() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let mut sub = arm(tmp.path().to_path_buf()).expect("arm");

		// Give notify a moment to register on the path before mutating.
		tokio::time::sleep(Duration::from_millis(200)).await;

		let target = tmp.path().join("hello.json");
		tokio::fs::write(&target, b"{}").await.expect("write");

		tokio::time::timeout(Duration::from_secs(3), sub.recv())
			.await
			.expect("recv timed out")
			.expect("channel closed");
	}
}
