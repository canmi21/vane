//! Daemon-side wrapper around [`notify_twophase`]. The lib owns the
//! `notify-debouncer-full` plumbing and the reload-worthy event
//! filter; this module wires the daemon's reload pipeline into a
//! `Subscription::recv` loop.
//!
//! ## Two-phase startup
//!
//! Initial subscription to `FSEvents` must complete **before** the
//! daemon's listeners become reachable on their bound ports — once
//! the public surface is up, the operator can drop a new rule file
//! in `<config_dir>/rules/` and rightly expects it to take effect.
//! If we subscribed late (after `listeners.start`), that drop's fs
//! event could fire in the gap and be lost (`FSEvents` on macOS does
//! not replay events for files that already exist at subscription
//! time).
//!
//! [`arm_watcher_subscription`] runs the synchronous half early,
//! BEFORE listener bind. Returned events queue into the lib's
//! unbounded mpsc; [`spawn_watcher_handler`] drains them later, after
//! the daemon is fully initialised.
//!
//! See `spec/crates/engine.md` § _Hot reload_.

use std::path::PathBuf;
use std::sync::Arc;

use notify_twophase::Subscription;
use tokio_util::sync::CancellationToken;
use vane_core::FlowLogSink;
use vane_engine::ListenerSet;
use vane_engine::VerbosityState;

use crate::reload::{ReloadCtx, ReloadOutcome, reload_once};

/// Bundle the file-watcher's reload loop needs: the shared reload
/// pipeline state plus the listener set + log/verbosity it must
/// reconcile against post-swap. Built once at boot and shared via
/// `Arc<WatcherCtx>` (along with [`Arc<ReloadCtx>`] inside).
pub(crate) struct WatcherCtx {
	pub reload: Arc<ReloadCtx>,
	pub listeners: Arc<ListenerSet>,
	pub verbosity: Arc<VerbosityState>,
	pub log_sink: Arc<dyn FlowLogSink>,
}

/// Phase 1 — subscribe to `FSEvents` synchronously. Must be called
/// **before** `ListenerSet::start` so the subscription is live by
/// the time the daemon's bound ports are reachable; events landing
/// in the gap before the handler task drains them queue into the
/// underlying unbounded mpsc.
///
/// # Errors
///
/// Propagates `notify::Error` from the underlying watcher backend
/// (typically permission-denied on the directory).
pub(crate) fn arm_watcher_subscription(
	config_dir: PathBuf,
) -> Result<Subscription, notify_twophase::notify::Error> {
	notify_twophase::arm(config_dir)
}

/// Phase 2 — spawn the tokio task that drains queued reload signals
/// into [`reload_once`] + `ListenerSet::reconcile`. Consumes the
/// subscription returned by [`arm_watcher_subscription`].
pub(crate) fn spawn_watcher_handler(
	sub: Subscription,
	ctx: Arc<WatcherCtx>,
	cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
	let mut sub = sub;

	tokio::spawn(async move {
		loop {
			tokio::select! {
				biased;
				() = cancel.cancelled() => {
					tracing::debug!("watcher: cancel received");
					return;
				}
				evt = sub.recv() => {
					if evt.is_none() {
						return;
					}
					// Serialize against the mgmt `reload` verb: both
					// sources call `reload_once + reconcile`, and
					// `ListenerSet::reconcile` mutates shared listener
					// state. The lock keeps the full pipeline atomic.
					let _guard = ctx.reload.run_lock.lock().await;
					let outcome = reload_once(&ctx.reload).await;
					match outcome {
						Ok(ReloadOutcome::Swapped { hash }) => {
							tracing::info!(
								hash = %hex32(&hash), "reloaded — flow graph swapped",
							);
							// Bring the listener set up to date with the
							// new graph's `entries`: bind any added
							// addresses, background-drain any removed
							// ones. Unchanged addresses are picked up by
							// the existing per-accept entry lookup.
							ctx.listeners.reconcile(&ctx.reload.graph, &ctx.verbosity, &ctx.log_sink);
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

#[cfg(test)]
mod tests {
	use std::fs;
	use std::time::{Duration, Instant};

	use arc_swap::ArcSwap;
	use vane_engine::SecurityConfig;
	use vane_engine::factories::{FetchFactories, MiddlewareFactories};
	use vane_engine::fetch::{http_proxy, http_synthesize, l4_forward};
	use vane_engine::flow_graph::FlowGraph;
	use vane_engine::middleware::{forward_client_ip, host_header_match, method_match, path_prefix};

	use super::*;
	use crate::providers::MetadataProviders;

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

	fn make_watcher_ctx(
		dir: &std::path::Path,
		swap: Arc<ArcSwap<FlowGraph>>,
		mw: Arc<MiddlewareFactories>,
		fetch: Arc<FetchFactories>,
	) -> Arc<WatcherCtx> {
		let reload = Arc::new(ReloadCtx {
			config_dir: dir.to_path_buf(),
			graph: swap,
			mw_factories: mw,
			fetch_factories: fetch,
			security_cfg: Arc::new(SecurityConfig::default()),
			plugin_registry: None,
			#[cfg(feature = "wasm")]
			wasm_dir: dir.join("wasm"),
			#[cfg(feature = "wasm")]
			wasm_runtime: None,
			#[cfg(feature = "wasm")]
			plugin_policies: None,
			#[cfg(feature = "acme")]
			acme_registry: None,
			run_lock: tokio::sync::Mutex::new(()),
		});
		Arc::new(WatcherCtx {
			reload,
			listeners: Arc::new(ListenerSet::new()),
			verbosity: Arc::new(VerbosityState::new()),
			log_sink: Arc::new(NullSink) as Arc<dyn vane_core::FlowLogSink>,
		})
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
		let watcher_ctx = make_watcher_ctx(tmp.path(), Arc::clone(&swap), mw, fetch);

		let sub = arm_watcher_subscription(tmp.path().to_path_buf()).expect("watcher init");
		let _handle = spawn_watcher_handler(sub, watcher_ctx, cancel.clone());

		// Give notify a moment to register on the path before mutating.
		tokio::time::sleep(Duration::from_millis(200)).await;

		// Edit the rule body — should produce a different version_hash.
		fs::write(tmp.path().join("rules").join("site.json"), rule_body(40100, "v2")).unwrap();

		// Poll up to 3s for the swap to land. Debounce is 250ms; reload
		// is typically <50ms; CI scheduling jitter accounts for the rest.
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
		let watcher_ctx = make_watcher_ctx(tmp.path(), swap, mw, fetch);

		let sub = arm_watcher_subscription(tmp.path().to_path_buf()).expect("watcher init");
		let handle = spawn_watcher_handler(sub, watcher_ctx, cancel.clone());

		cancel.cancel();
		// Watcher task should join within 1s after cancellation.
		let join_result = tokio::time::timeout(Duration::from_secs(1), handle).await;
		assert!(join_result.is_ok(), "watcher did not join within 1s after cancel");
	}
}
