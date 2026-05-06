//! File watcher: `notify-debouncer-full` observes the config directory
//! for ~250ms-debounced batches; each reload-worthy batch triggers one
//! [`reload_once`] call. Watcher lifetime is bound to a
//! [`CancellationToken`].
//!
//! `notify-debouncer-full`'s callback runs in a sync context; we bridge
//! it into tokio via an unbounded mpsc channel. Each debounced batch
//! that contains at least one reload-worthy event (file create / modify
//! data / rename / remove under the watched tree, per
//! `spec/architecture/09-config.md` § _Watched events_) coalesces to a
//! single `()` send. Metadata-only / access / unknown events are
//! filtered out so reload CPU is spent only on real config changes.
//!
//! ## Two-phase startup
//!
//! Initial subscription to `FSEvents` must complete **before** the
//! daemon's listeners become reachable on their bound ports — once
//! the public surface is up, the operator can drop a new rule file in
//! `<config_dir>/rules/` and rightly expects it to take effect. If
//! we subscribed late (after listeners.start), that drop's fs event
//! could fire in the gap and be lost (`FSEvents` on macOS does not
//! replay events for files that already exist at subscription time).
//!
//! [`arm_watcher_subscription`] runs the `new_debouncer` +
//! `debouncer.watch()` synchronous half early, BEFORE listener bind.
//! Returned events queue into an unbounded mpsc that
//! [`spawn_watcher_handler`] drains later, after the daemon is fully
//! initialised — including when a watched event lands during the gap.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use notify::RecursiveMode;
use notify::event::{EventKind, ModifyKind};
use notify_debouncer_full::{
	DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};
use tokio_util::sync::CancellationToken;
use vane_core::FlowLogSink;
use vane_engine::ListenerSet;
use vane_engine::SecurityConfig;
use vane_engine::VerbosityState;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::FlowGraph;

use crate::reload::{ReloadOutcome, reload_once};

/// Concrete debouncer type; the platform-recommended watcher backend
/// (`FSEvents` on macOS, inotify on Linux) plus the recommended cache.
type WatchHandle = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

/// Active `FSEvents` subscription with its companion mpsc receiver.
/// Held by the caller across the listener-bind window so events
/// landing in the gap are queued, not lost.
pub(crate) struct WatcherSubscription {
	debouncer: WatchHandle,
	rx: tokio::sync::mpsc::UnboundedReceiver<()>,
	config_dir: PathBuf,
}

/// Window over which fs events are coalesced before triggering a
/// reload. 250ms matches `spec/architecture/09-config.md` § _Reload_.
const DEBOUNCE_MS: u64 = 250;

/// Phase 1 — build the debouncer and subscribe to `FSEvents`
/// synchronously. Must be called **before** `ListenerSet::start` so
/// the subscription is live by the time the daemon's bound ports are
/// reachable; events landing in the gap before the handler task
/// drains them queue into the unbounded mpsc.
///
/// # Errors
/// Propagates `notify::Error` from the underlying watcher backend
/// (typically permission-denied on the directory).
pub(crate) fn arm_watcher_subscription(
	config_dir: PathBuf,
) -> Result<WatcherSubscription, notify::Error> {
	let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();

	// Canonicalize the watch root so `starts_with` in the event filter
	// matches what notify reports. macOS's FSEvents returns paths under
	// `/private/var/folders/...` while a `tempfile::tempdir()` may give
	// the symlinked `/var/folders/...` form; without canonicalization
	// the prefix check rejects every legitimate event.
	let watch_root = config_dir.canonicalize().unwrap_or_else(|_| config_dir.clone());
	let mut debouncer =
		new_debouncer(Duration::from_millis(DEBOUNCE_MS), None, move |res: DebounceEventResult| {
			let Ok(events) = res else { return };
			if is_reloadable_batch(&events, &watch_root) {
				// Coalesce: a single () per debounce window. Receiver is
				// unbounded so send is sync-ok.
				let _ = tx.send(());
			}
		})?;
	debouncer.watch(&config_dir, RecursiveMode::Recursive)?;

	Ok(WatcherSubscription { debouncer, rx, config_dir })
}

/// Phase 2 — spawn the tokio task that drains queued reload signals
/// into [`reload_once`] + `ListenerSet::reconcile`. Consumes the
/// subscription returned by [`arm_watcher_subscription`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_watcher_handler(
	sub: WatcherSubscription,
	graph: Arc<ArcSwap<FlowGraph>>,
	listeners: Arc<ListenerSet>,
	verbosity: Arc<VerbosityState>,
	log_sink: Arc<dyn FlowLogSink>,
	mw_factories: Arc<MiddlewareFactories>,
	fetch_factories: Arc<FetchFactories>,
	security_cfg: Arc<SecurityConfig>,
	plugin_registry: Option<Arc<arc_swap::ArcSwap<vane_engine::flow_graph::PluginRegistry>>>,
	#[cfg(feature = "wasm")] plugin_policies: Option<
		Arc<arc_swap::ArcSwap<vane_core::PluginPolicyTable>>,
	>,
	#[cfg(feature = "wasm")] wasm_runtime: Option<Arc<vane_wasm::WasmtimeRuntime>>,
	#[cfg(feature = "wasm")] wasm_dir: Option<std::path::PathBuf>,
	cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
	let WatcherSubscription { debouncer, mut rx, config_dir } = sub;

	tokio::spawn(async move {
		// `_debouncer` is held here so the underlying RecommendedWatcher
		// and its internal thread stay alive for the task's lifetime.
		// Dropping the binding stops watching.
		let _debouncer = debouncer;

		loop {
			tokio::select! {
				biased;
				() = cancel.cancelled() => {
					tracing::debug!("watcher: cancel received");
					return;
				}
				evt = rx.recv() => {
					if evt.is_none() {
						return;
					}
					let outcome = reload_once(
						&config_dir,
						#[cfg(feature = "wasm")]
						wasm_dir.as_deref(),
						#[cfg(feature = "wasm")]
						wasm_runtime.as_ref(),
						&graph,
						&mw_factories,
						&fetch_factories,
						&security_cfg,
						plugin_registry.as_ref(),
						#[cfg(feature = "wasm")]
						plugin_policies.as_ref(),
					)
					.await;
					match outcome {
						Ok(ReloadOutcome::Swapped { hash }) => {
							tracing::info!(
								hash = %hex32(&hash), "reloaded — flow graph swapped",
							);
							// Bring the listener set up to date with the new
							// graph's `entries`: bind any added addresses,
							// background-drain any removed ones. Unchanged
							// addresses are picked up by the existing
							// per-accept entry lookup.
							listeners.reconcile(
								Arc::clone(&graph),
								Arc::clone(&verbosity),
								Arc::clone(&log_sink),
							);
						}
						Ok(ReloadOutcome::Unchanged { .. }) => tracing::debug!(
							"reloaded — no semantic change, swap skipped",
						),
						Err(e) => tracing::error!(
							error = %e, "reload failed; active graph unchanged",
						),
					}
				}
			}
		}
	})
}

fn hex32(bytes: &[u8; 32]) -> String {
	use std::fmt::Write as _;
	let mut s = String::with_capacity(64);
	for b in bytes {
		let _ = write!(s, "{b:02x}");
	}
	s
}

/// Whether a debounced batch contains at least one event that warrants
/// recompiling the rule set. Per
/// `spec/architecture/09-config.md` § _Watched events_, only file-level
/// mutations under the watched tree are reload-worthy:
///
/// - `Create(_)` — a new rule file appeared.
/// - `Modify(Data(_))` — content was rewritten in place.
/// - `Modify(Name(_))` — atomic editor save (write to `.tmp` →
///   rename), and analogous file moves.
/// - `Remove(_)` — a rule file was deleted.
///
/// Events filtered out:
///
/// - `Access(_)` — atime / open / close, never affects rule semantics.
/// - `Modify(Metadata(_))` — chmod / chown / utime, no content change.
/// - `Modify(Other | Any)` and the top-level `Other` / `Any` — kept
///   conservative: backends differ, and `version_hash` idempotency in
///   `reload_once` is the safety net if a real edit ever surfaces with
///   a fuzzy classification.
///
/// Path filter: at least one of the event's paths must live under
/// `watch_root` so stray events from siblings on the same filesystem
/// don't drive reloads. `notify`'s recursive watch mostly handles this
/// at the kernel level, but the path check is cheap and defends
/// against backends that occasionally bubble up adjacent traffic.
pub(crate) fn is_reloadable_batch(events: &[DebouncedEvent], watch_root: &Path) -> bool {
	events.iter().any(|debounced| {
		is_reloadable_kind(debounced.event.kind)
			&& debounced.event.paths.iter().any(|p| p.starts_with(watch_root))
	})
}

fn is_reloadable_kind(kind: EventKind) -> bool {
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
	use std::fs;
	use std::time::Instant;

	use notify::event::{
		AccessKind, CreateKind, Event as NotifyEvent, ModifyKind, RemoveKind, RenameMode,
	};
	use notify_debouncer_full::DebouncedEvent as DEvent;
	use vane_engine::fetch::{http_proxy, http_synthesize, l4_forward};
	use vane_engine::flow_graph::FlowGraph;
	use vane_engine::middleware::{forward_client_ip, host_header_match, method_match, path_prefix};

	use super::*;
	use crate::providers::MetadataProviders;

	// ----- pure helper: is_reloadable_batch -------------------------------

	fn ev_under(root: &Path, kind: EventKind) -> DEvent {
		let event = NotifyEvent::new(kind).add_path(root.join("rules").join("foo.json"));
		DEvent::new(event, Instant::now())
	}

	fn ev_outside(kind: EventKind) -> DEvent {
		let event = NotifyEvent::new(kind).add_path(std::path::PathBuf::from("/elsewhere/file.json"));
		DEvent::new(event, Instant::now())
	}

	#[test]
	fn event_filter_accepts_create_modify_data_rename_remove() {
		let root = std::path::PathBuf::from("/tmp/vane-cfg-fixture");
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
	fn event_filter_rejects_metadata_access_and_unknown() {
		let root = std::path::PathBuf::from("/tmp/vane-cfg-fixture");
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
	fn event_filter_rejects_event_outside_watch_root() {
		// Even a clean file-create event should not trigger a reload if its
		// path is not under the watched tree.
		let root = std::path::PathBuf::from("/tmp/vane-cfg-fixture");
		let batch = vec![ev_outside(EventKind::Create(CreateKind::File))];
		assert!(!is_reloadable_batch(&batch, &root));
	}

	#[test]
	fn event_filter_accepts_when_at_least_one_event_qualifies() {
		// Mixed batch: a metadata event alone wouldn't trigger; pairing it
		// with a create event under the watch root must.
		let root = std::path::PathBuf::from("/tmp/vane-cfg-fixture");
		let batch = vec![
			ev_under(
				&root,
				EventKind::Modify(ModifyKind::Metadata(notify::event::MetadataKind::Permissions)),
			),
			ev_under(&root, EventKind::Create(CreateKind::File)),
		];
		assert!(is_reloadable_batch(&batch, &root));
	}

	// ----- end-to-end watcher integration ---------------------------------

	fn build_factories() -> (Arc<MiddlewareFactories>, Arc<FetchFactories>) {
		let mut mw = MiddlewareFactories::new();
		host_header_match::register(&mut mw);
		path_prefix::register(&mut mw);
		method_match::register(&mut mw);
		forward_client_ip::register(&mut mw);
		let mut fetch = FetchFactories::new();
		l4_forward::register(&mut fetch);
		http_proxy::register(&mut fetch, None);
		http_synthesize::register(&mut fetch);
		(Arc::new(mw), Arc::new(fetch))
	}

	fn rule_body(port: u16, body: &str) -> String {
		format!(
			r#"{{
				"rules": [{{
					"preset": "static_site",
					"name": "site",
					"listen": ["127.0.0.1:{port}"],
					"args": {{ "status": 200, "body": "{body}" }}
				}}]
			}}"#
		)
	}

	fn initial_graph(dir: &std::path::Path) -> Arc<FlowGraph> {
		let loaded = vane_core::config::load(dir).expect("load");
		let providers = MetadataProviders::new();
		let symbolic =
			vane_core::compile::compile(loaded.files, &providers, &providers).expect("compile");
		let (mw, fetch) = build_factories();
		FlowGraph::link(symbolic, &mw, &fetch).expect("link")
	}

	/// In-memory `FlowLogSink` used by the watcher integration tests —
	/// the watcher passes one through to `reconcile`, which never
	/// actually emits to it, so the impl is a no-op.
	struct NullSink;
	impl vane_core::FlowLogSink for NullSink {
		fn emit(&self, _event: vane_core::FlowLogEvent) {}
	}

	fn watcher_extras() -> (Arc<ListenerSet>, Arc<VerbosityState>, Arc<dyn vane_core::FlowLogSink>) {
		let listeners = Arc::new(ListenerSet::new());
		let verbosity = Arc::new(VerbosityState::new());
		let sink: Arc<dyn vane_core::FlowLogSink> = Arc::new(NullSink);
		(listeners, verbosity, sink)
	}

	#[tokio::test]
	async fn watcher_triggers_reload_on_rule_file_write() {
		let tmp = tempfile::tempdir().expect("tempdir");
		fs::create_dir(tmp.path().join("rules")).unwrap();
		fs::write(tmp.path().join("rules").join("site.json"), rule_body(40100, "v1")).unwrap();

		let initial = initial_graph(tmp.path());
		let h0 = initial.meta().version_hash;
		let swap = Arc::new(ArcSwap::new(initial));
		let (mw, fetch) = build_factories();
		let cancel = CancellationToken::new();
		let (listeners, verbosity, sink) = watcher_extras();

		let sub = arm_watcher_subscription(tmp.path().to_path_buf()).expect("watcher init");
		let _handle = spawn_watcher_handler(
			sub,
			Arc::clone(&swap),
			listeners,
			verbosity,
			sink,
			mw,
			fetch,
			Arc::new(SecurityConfig::default()),
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			cancel.clone(),
		);

		// Give notify a moment to register on the path before mutating.
		tokio::time::sleep(Duration::from_millis(200)).await;

		// Edit the rule body — should produce a different version_hash.
		fs::write(tmp.path().join("rules").join("site.json"), rule_body(40100, "v2")).unwrap();

		// Poll up to 3s for the swap to land. Debounce is 250ms; reload is
		// typically <50ms; CI scheduling jitter accounts for the rest.
		let deadline = Instant::now() + Duration::from_secs(3);
		while Instant::now() < deadline {
			if swap.load().meta().version_hash != h0 {
				cancel.cancel();
				return;
			}
			tokio::time::sleep(Duration::from_millis(50)).await;
		}
		panic!("watcher did not propagate edit within 3s");
	}

	#[tokio::test]
	async fn watcher_cancels_cleanly() {
		let tmp = tempfile::tempdir().expect("tempdir");
		fs::create_dir(tmp.path().join("rules")).unwrap();
		fs::write(tmp.path().join("rules").join("site.json"), rule_body(40101, "x")).unwrap();

		let initial = initial_graph(tmp.path());
		let swap = Arc::new(ArcSwap::new(initial));
		let (mw, fetch) = build_factories();
		let cancel = CancellationToken::new();
		let (listeners, verbosity, sink) = watcher_extras();

		let sub = arm_watcher_subscription(tmp.path().to_path_buf()).expect("watcher init");
		let handle = spawn_watcher_handler(
			sub,
			swap,
			listeners,
			verbosity,
			sink,
			mw,
			fetch,
			Arc::new(SecurityConfig::default()),
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			cancel.clone(),
		);

		cancel.cancel();
		// Watcher task should join within 1s after cancellation.
		let join_result = tokio::time::timeout(Duration::from_secs(1), handle).await;
		assert!(join_result.is_ok(), "watcher did not join within 1s after cancel");
	}
}
