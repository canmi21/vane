//! File watcher: `notify-debouncer-full` observes the config directory
//! for ~250ms-debounced batches; each batch triggers one
//! [`reload_once`] call. Watcher lifetime is bound to a
//! [`CancellationToken`].
//!
//! `notify-debouncer-full`'s callback runs in a sync context; we bridge
//! it into tokio via an unbounded mpsc channel. Each debounced batch
//! coalesces to a single `()` send — the receiver doesn't care which
//! file changed, only that *something* changed.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use notify::RecursiveMode;
use notify_debouncer_full::new_debouncer;
use tokio_util::sync::CancellationToken;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::FlowGraph;

use crate::reload::{ReloadOutcome, reload_once};

/// Window over which fs events are coalesced before triggering a
/// reload. 250ms matches `spec/architecture/09-config.md` § _Reload_.
const DEBOUNCE_MS: u64 = 250;

/// Spawn a tokio task that watches `config_dir` recursively. Each
/// debounced batch of fs events triggers one [`reload_once`] call.
/// Cancel the supplied [`CancellationToken`] to stop the watcher
/// cleanly; the task drops the underlying `notify::Watcher` and
/// returns.
///
/// # Errors
/// Returns `notify::Error` when initial watcher / debouncer
/// construction fails (typically permission-denied at the directory
/// level). The daemon logs and continues without auto-reload — there's
/// no useful retry; the operator has to fix the underlying problem
/// before a daemon restart picks up the watcher again.
pub(crate) fn spawn_watcher(
	config_dir: PathBuf,
	graph: Arc<ArcSwap<FlowGraph>>,
	mw_factories: Arc<MiddlewareFactories>,
	fetch_factories: Arc<FetchFactories>,
	cancel: CancellationToken,
) -> Result<tokio::task::JoinHandle<()>, notify::Error> {
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

	// Build the debouncer in the calling thread so any setup error
	// surfaces synchronously to the caller. The Debouncer's internal
	// thread + RecommendedWatcher run in the background; we hold the
	// Debouncer in the spawned tokio task to keep them alive for the
	// task's lifetime.
	let mut debouncer = new_debouncer(
		Duration::from_millis(DEBOUNCE_MS),
		None,
		move |res: notify_debouncer_full::DebounceEventResult| {
			if res.is_ok() {
				// Coalesce: a single () per debounce window. Receiver is
				// unbounded so send is sync-ok.
				let _ = tx.send(());
			}
		},
	)?;
	debouncer.watch(&config_dir, RecursiveMode::Recursive)?;

	let handle = tokio::spawn(async move {
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
					match reload_once(&config_dir, &graph, &mw_factories, &fetch_factories) {
						Ok(ReloadOutcome::Swapped { hash }) => tracing::info!(
							hash = %hex32(&hash), "reloaded — flow graph swapped",
						),
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
	});

	Ok(handle)
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
	use std::time::Instant;

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
		http_proxy::register(&mut fetch);
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
		let providers = MetadataProviders;
		let symbolic =
			vane_core::compile::compile(loaded.files, &providers, &providers).expect("compile");
		let (mw, fetch) = build_factories();
		FlowGraph::link(symbolic, &mw, &fetch).expect("link")
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

		let _handle =
			spawn_watcher(tmp.path().to_path_buf(), Arc::clone(&swap), mw, fetch, cancel.clone())
				.expect("watcher init");

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

		let handle = spawn_watcher(tmp.path().to_path_buf(), swap, mw, fetch, cancel.clone())
			.expect("watcher init");

		cancel.cancel();
		// Watcher task should join within 1s after cancellation.
		let join_result = tokio::time::timeout(Duration::from_secs(1), handle).await;
		assert!(join_result.is_ok(), "watcher did not join within 1s after cancel");
	}
}
