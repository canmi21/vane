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

use arc_swap::ArcSwap;
use notify_twophase::Subscription;
use tokio_util::sync::CancellationToken;
use vane_core::FlowLogSink;
use vane_engine::ListenerSet;
use vane_engine::SecurityConfig;
use vane_engine::VerbosityState;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::FlowGraph;

use crate::reload::{ReloadOutcome, reload_once};

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
) -> Result<(Subscription, PathBuf), notify_twophase::notify::Error> {
	let sub = notify_twophase::arm(config_dir.clone())?;
	Ok((sub, config_dir))
}

/// Phase 2 — spawn the tokio task that drains queued reload signals
/// into [`reload_once`] + `ListenerSet::reconcile`. Consumes the
/// subscription returned by [`arm_watcher_subscription`].
pub(crate) fn spawn_watcher_handler(
	sub: Subscription,
	config_dir: PathBuf,
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
	#[cfg(feature = "acme")] acme_registry: Option<Arc<vane_engine::acme::ManagedCertRegistry>>,
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
						#[cfg(feature = "acme")]
						acme_registry.as_ref(),
					)
					.await;
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
							listeners.reconcile(&graph, &verbosity, &log_sink);
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

		let (sub, dir) = arm_watcher_subscription(tmp.path().to_path_buf()).expect("watcher init");
		let _handle = spawn_watcher_handler(
			sub,
			dir,
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
			#[cfg(feature = "acme")]
			None,
			cancel.clone(),
		);

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
		let (listeners, verbosity, sink) = watcher_extras();

		let (sub, dir) = arm_watcher_subscription(tmp.path().to_path_buf()).expect("watcher init");
		let handle = spawn_watcher_handler(
			sub,
			dir,
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
			#[cfg(feature = "acme")]
			None,
			cancel.clone(),
		);

		cancel.cancel();
		// Watcher task should join within 1s after cancellation.
		let join_result = tokio::time::timeout(Duration::from_secs(1), handle).await;
		assert!(join_result.is_ok(), "watcher did not join within 1s after cancel");
	}
}
