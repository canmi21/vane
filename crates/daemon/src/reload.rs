//! Reload pipeline: re-run `vane_core::compile` on the live config dir,
//! re-link against the engine factories, and `ArcSwap::store` the new
//! graph atomically.
//!
//! In-flight connections retain their captured `Arc<FlowGraph>` (per
//! `crates/engine/src/listener.rs::run_accept_loop`) and run to natural
//! completion against the old graph; new accepted connections see the
//! swapped-in graph immediately. Failure at any stage (config load,
//! compile, link) leaves the active graph completely unchanged.
//!
//! Idempotency: `FlowGraphMeta::version_hash` is a SHA-256 over the
//! canonical rule set (02-flow.md § _`FlowGraph` metadata_). When a
//! recompile reproduces the same hash — typical for `cp -p` mtime
//! bumps or whitespace-only edits — the swap is skipped.

use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use vane_core::Error;
#[cfg(feature = "wasm")]
use vane_core::PluginPolicyTable;
use vane_core::compile::compile;
use vane_engine::SecurityConfig;
use vane_engine::factories::{FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FlowGraph, PluginRegistry};
#[cfg(feature = "wasm")]
use vane_wasm::WasmtimeRuntime;

use crate::providers::MetadataProviders;
#[cfg(feature = "wasm")]
use crate::wasm_loader;

/// Outcome of a single reload attempt. Both successful variants carry
/// the post-link `version_hash` so observers (mgmt API, eventually) can
/// correlate reload events with on-disk config changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReloadOutcome {
	/// Recompile succeeded but `version_hash` matched the active graph;
	/// the swap was skipped. Typical for benign `cp -p` mtime bumps,
	/// touch-only edits, or whitespace changes.
	Unchanged { hash: [u8; 32] },
	/// Recompile succeeded and the new graph was atomically stored.
	Swapped { hash: [u8; 32] },
}

/// Reload once: WASM rescan → load → compile → link → swap. On any
/// error, returns `Err` *without* touching the active graph.
///
/// The WASM phase reconciles `<wasm_dir>/*.wasm` against the runtime
/// (see [`crate::wasm_loader::reload_dir`]). When WASM bytes change in
/// a metadata-compatible way, the swap happens silently — the active
/// graph stays valid because `MiddlewareInst::Wasm` keeps its existing
/// `Arc<PluginMetadata>` and the runtime's `Component` is what changes.
/// Metadata-incompatible WASM changes / additions / deletions force a
/// rule recompile (`schema_changed = true`) so the new metadata
/// threads into the freshly-built graph.
///
/// # Errors
/// Surfaces whatever failed: filesystem (`config::load`), compile
/// (preset / merge / lower / validate), link (factory rejection,
/// kind mismatch, feature-disabled), or WASM rescan (dir unreadable,
/// component validation failure).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reload_once(
	config_dir: &Path,
	#[cfg(feature = "wasm")] wasm_dir: Option<&Path>,
	#[cfg(feature = "wasm")] runtime: Option<&Arc<WasmtimeRuntime>>,
	graph: &ArcSwap<FlowGraph>,
	mw_factories: &MiddlewareFactories,
	fetch_factories: &FetchFactories,
	security_cfg: &Arc<SecurityConfig>,
	plugin_registry: Option<&Arc<ArcSwap<PluginRegistry>>>,
	#[cfg(feature = "wasm")] plugin_policies: Option<&Arc<ArcSwap<PluginPolicyTable>>>,
	#[cfg(feature = "acme")] acme_registry: Option<&Arc<vane_engine::acme::ManagedCertRegistry>>,
) -> Result<ReloadOutcome, Error> {
	#[cfg(feature = "wasm")]
	let wasm_outcome = match (wasm_dir, runtime, plugin_registry, plugin_policies) {
		(Some(dir), Some(rt), Some(reg), Some(pol)) => {
			Some(wasm_loader::reload_dir(dir, rt, reg, pol).await?)
		}
		_ => None,
	};
	#[cfg(not(feature = "wasm"))]
	let wasm_outcome: Option<wasm_loader_stub::WasmReloadOutcome> = None;

	let loaded = vane_core::config::load(config_dir)?;
	let registry_snap = plugin_registry.map(|r| r.load_full());
	let providers = match registry_snap.as_ref() {
		#[cfg(feature = "wasm")]
		Some(reg) => MetadataProviders::with_plugins(Arc::clone(reg)),
		#[cfg(not(feature = "wasm"))]
		Some(_) => MetadataProviders::new(),
		None => MetadataProviders::new(),
	};
	let symbolic = compile(loaded.files, &providers, &providers)?;

	// Pre-link CRL refresh: register any newly-named source with the
	// daemon-wide cache so the upcoming `link` and subsequent handshakes
	// see fresh bytes. URL sources already registered are left to the
	// background refresher; file sources always re-read (spec § _CRL
	// checking_, file source reload semantics).
	if let Some(cache) = &security_cfg.crl_cache {
		let listener_sources =
			vane_engine::tls::collect_listener_crl_sources(&symbolic.meta.listener_tls);
		let upstream_sources = vane_engine::tls::collect_upstream_crl_sources(&symbolic);
		let sources =
			vane_engine::tls::dedupe_crl_sources(listener_sources.into_iter().chain(upstream_sources));
		if !sources.is_empty() {
			cache.ensure_loaded_new(&sources).map_err(|e| Error::compile(format!("crl reload: {e}")))?;
		}
	}

	let new_graph = {
		#[cfg(feature = "acme")]
		{
			FlowGraph::link_with_acme(
				symbolic,
				mw_factories,
				registry_snap.as_deref(),
				fetch_factories,
				Arc::clone(security_cfg),
				acme_registry,
			)
			.map_err(|e| Error::compile(format!("link: {e}")))?
		}
		#[cfg(not(feature = "acme"))]
		{
			match registry_snap.as_ref() {
				Some(reg) => FlowGraph::link_with_plugins(
					symbolic,
					mw_factories,
					reg,
					fetch_factories,
					Arc::clone(security_cfg),
				),
				None => FlowGraph::link_with_security(
					symbolic,
					mw_factories,
					fetch_factories,
					Arc::clone(security_cfg),
				),
			}
			.map_err(|e| Error::compile(format!("link: {e}")))?
		}
	};

	let new_hash = new_graph.meta().version_hash;
	let active_hash = graph.load().meta().version_hash;
	// `version_hash` covers the canonical rule set but not plugin
	// metadata; if WASM schema changed we force a swap so
	// `MiddlewareInst::Wasm` carries the new metadata Arc. Follow-up:
	// extend `FlowGraphMeta::version_hash` to cover plugin metadata
	// so this short-circuit goes away.
	let force_swap = wasm_outcome.as_ref().is_some_and(|o| o.schema_changed);
	if active_hash == new_hash && !force_swap {
		return Ok(ReloadOutcome::Unchanged { hash: new_hash });
	}
	graph.store(new_graph);
	Ok(ReloadOutcome::Swapped { hash: new_hash })
}

#[cfg(not(feature = "wasm"))]
mod wasm_loader_stub {
	pub(crate) struct WasmReloadOutcome {
		pub schema_changed: bool,
	}
}

#[cfg(test)]
mod tests {
	use std::fs;
	use std::sync::Arc;

	use vane_engine::fetch::{http_proxy, http_synthesize, l4_forward};
	use vane_engine::middleware::{forward_client_ip, host_header_match, method_match, path_prefix};

	use super::*;

	fn default_security() -> Arc<vane_engine::SecurityConfig> {
		Arc::new(vane_engine::SecurityConfig::default())
	}

	fn build_factories() -> (MiddlewareFactories, FetchFactories) {
		let mut mw = MiddlewareFactories::new();
		host_header_match::register(&mut mw);
		path_prefix::register(&mut mw);
		method_match::register(&mut mw);
		forward_client_ip::register(&mut mw);
		let mut fetch = FetchFactories::new();
		l4_forward::register(&mut fetch);
		http_proxy::register(&mut fetch, None);
		http_synthesize::register(&mut fetch);
		(mw, fetch)
	}

	fn write_rule(dir: &Path, body: &str) {
		let rules = dir.join("rules");
		fs::create_dir_all(&rules).expect("create rules/");
		fs::write(rules.join("test.json"), body).expect("write rule");
	}

	fn rule_v1(port: u16, body: &str) -> String {
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

	fn initial_graph(dir: &Path) -> Arc<FlowGraph> {
		let loaded = vane_core::config::load(dir).expect("initial load");
		let providers = MetadataProviders::new();
		let symbolic =
			vane_core::compile::compile(loaded.files, &providers, &providers).expect("initial compile");
		let (mw, fetch) = build_factories();
		FlowGraph::link(symbolic, &mw, &fetch).expect("initial link")
	}

	#[tokio::test]
	async fn reload_once_swaps_when_rule_set_changes() {
		let tmp = tempfile::tempdir().expect("tempdir");
		write_rule(tmp.path(), &rule_v1(40001, "v1"));
		let initial = initial_graph(tmp.path());
		let h0 = initial.meta().version_hash;
		let swap = ArcSwap::new(initial);
		let (mw, fetch) = build_factories();

		write_rule(tmp.path(), &rule_v1(40001, "v2"));
		let outcome = reload_once(
			tmp.path(),
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			&swap,
			&mw,
			&fetch,
			&default_security(),
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "acme")]
			None,
		)
		.await
		.expect("reload");
		match outcome {
			ReloadOutcome::Swapped { hash } => assert_ne!(hash, h0, "hash must change with body"),
			ReloadOutcome::Unchanged { .. } => panic!("expected Swapped, got Unchanged"),
		}
		assert_ne!(swap.load().meta().version_hash, h0, "active graph hash updated");
	}

	#[tokio::test]
	async fn reload_once_skips_swap_when_unchanged() {
		let tmp = tempfile::tempdir().expect("tempdir");
		write_rule(tmp.path(), &rule_v1(40002, "stable"));
		let initial = initial_graph(tmp.path());
		let h0 = initial.meta().version_hash;
		let swap = ArcSwap::new(initial);
		let (mw, fetch) = build_factories();

		// Rewrite the same content — hash should match, swap should skip.
		write_rule(tmp.path(), &rule_v1(40002, "stable"));
		let outcome = reload_once(
			tmp.path(),
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			&swap,
			&mw,
			&fetch,
			&default_security(),
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "acme")]
			None,
		)
		.await
		.expect("reload");
		assert!(matches!(outcome, ReloadOutcome::Unchanged { hash } if hash == h0));
		assert_eq!(swap.load().meta().version_hash, h0);
	}

	#[tokio::test]
	async fn reload_once_compile_failure_keeps_active_graph() {
		let tmp = tempfile::tempdir().expect("tempdir");
		write_rule(tmp.path(), &rule_v1(40003, "v1"));
		let initial = initial_graph(tmp.path());
		let h0 = initial.meta().version_hash;
		let swap = ArcSwap::new(initial);
		let (mw, fetch) = build_factories();

		// Corrupt the file with invalid JSON.
		fs::write(tmp.path().join("rules").join("test.json"), "{ this is not json").unwrap();
		let err = reload_once(
			tmp.path(),
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			&swap,
			&mw,
			&fetch,
			&default_security(),
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "acme")]
			None,
		)
		.await
		.expect_err("must fail compile");
		assert!(err.to_string().contains("parse"));
		assert_eq!(swap.load().meta().version_hash, h0, "active graph untouched");
	}

	#[tokio::test]
	async fn reload_once_link_failure_keeps_active_graph() {
		// Use websocket fetch kind: registered in core's metadata provider
		// but no engine factory is registered for it in this test, so link
		// fails with UnknownFetch.
		let tmp = tempfile::tempdir().expect("tempdir");
		write_rule(tmp.path(), &rule_v1(40004, "ok"));
		let initial = initial_graph(tmp.path());
		let h0 = initial.meta().version_hash;
		let swap = ArcSwap::new(initial);

		// New rule references the websocket fetch kind, which has core
		// metadata but no factory in the link registry.
		fs::write(
			tmp.path().join("rules").join("test.json"),
			r#"{
				"rules": [{
					"name": "ws",
					"listen": ["127.0.0.1:40005"],
					"match": { "http.header.upgrade": { "equals": "websocket" } },
					"terminate": { "type": "websocket", "upstream": "127.0.0.1:9000" }
				}]
			}"#,
		)
		.unwrap();

		// Build factories WITHOUT registering websocket — that's the test fixture
		// shape used in production until ws lands.
		let (mw, fetch) = build_factories();
		let err = reload_once(
			tmp.path(),
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			&swap,
			&mw,
			&fetch,
			&default_security(),
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "acme")]
			None,
		)
		.await
		.expect_err("must fail link");
		assert!(err.to_string().to_lowercase().contains("link"));
		assert_eq!(swap.load().meta().version_hash, h0, "active graph untouched");
	}

	#[tokio::test]
	async fn reload_once_initial_swap_to_arcswap_works() {
		// Empty rules dir → graph compiles cleanly with zero entries.
		let tmp = tempfile::tempdir().expect("tempdir");
		fs::create_dir(tmp.path().join("rules")).unwrap();
		let initial = initial_graph(tmp.path());
		let swap = ArcSwap::new(initial);
		let (mw, fetch) = build_factories();

		// Add a rule for the first time — the swap-once path.
		write_rule(tmp.path(), &rule_v1(40006, "first"));
		let outcome = reload_once(
			tmp.path(),
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "wasm")]
			None,
			&swap,
			&mw,
			&fetch,
			&default_security(),
			None,
			#[cfg(feature = "wasm")]
			None,
			#[cfg(feature = "acme")]
			None,
		)
		.await
		.expect("reload");
		assert!(matches!(outcome, ReloadOutcome::Swapped { .. }));
		// `127.0.0.1:N` is v4-only — `:N` shorthand would expand to both v4 + v6.
		assert_eq!(swap.load().symbolic().entries.len(), 1, "single v4 entry for 127.0.0.1:40006");
	}
}
