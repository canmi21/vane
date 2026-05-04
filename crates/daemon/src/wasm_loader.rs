//! Boot-time WASM module discovery, instantiation, and registry build.
//!
//! Per `spec/architecture/11-wasm.md` § _Module lifecycle_, the daemon
//! scans `<config_dir>/wasm/*.wasm` once at startup and loads every
//! component it can validate. This module owns that scan + the lazy
//! [`WasmtimeRuntime`] instantiation:
//!
//! * Empty / missing dir → return `None`. No runtime is constructed,
//!   so the 1 ms epoch ticker doesn't run for daemons that have no
//!   plugins.
//! * Any single module failure is **independent**: WARN-logged and
//!   the loader continues with the rest. The first successful load
//!   triggers runtime construction (idempotent — at most one runtime
//!   per daemon process).
//! * If every load fails, the runtime is dropped and `None` is
//!   returned, restoring the no-tick posture.
//!
//! Plugin registration uses the file stem as the `<module>` half of
//! the rule reference (`<module>:<export>`). Spec exemplifies
//! `auth-bundle:jwt-validator` for a plugin whose `metadata.name` is
//! `auth-bundle`; the daemon maps to file stem instead so:
//!
//! * Operators don't have to load the module to know its rule name.
//! * Two plugins reusing the same `metadata.name` cannot collide.
//! * Refactoring a `.wasm` (renaming, splitting) is a deliberate
//!   operator action — it shows up as a filesystem rename rather
//!   than a silent change of in-rule reference.
//!
//! The watcher / hot-reload path (out of scope for this commit) will
//! reuse this loader's output: re-running the scan and rebuilding
//! the registry. For now reload reuses whatever the boot scan
//! produced; adding new plugins requires a daemon restart.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vane_core::{HttpFetchBackend, ModuleId, PluginMetadata, PluginPolicyTable, WasmRuntime};
use vane_engine::flow_graph::PluginRegistry;
use vane_engine::wasm_fetch::HyperHttpFetchBackend;
use vane_wasm::WasmtimeRuntime;

/// Outcome of [`load_all`] when at least one `.wasm` was loaded.
pub(crate) struct LoadedWasm {
	pub runtime: Arc<WasmtimeRuntime>,
	pub registry: Arc<PluginRegistry>,
	/// Operator-owned per-plugin policy table loaded from
	/// `<wasm_dir>/policy.json`. Empty when the file is absent —
	/// every plugin then resolves to `PluginHttpPolicy::default`
	/// (deny-all).
	#[allow(dead_code, reason = "consumed by HostState wiring in the next commit")]
	pub policies: Arc<PluginPolicyTable>,
	#[allow(dead_code, reason = "diagnostic surface for future hot-reload work")]
	pub modules: Vec<LoadedModuleInfo>,
}

#[allow(dead_code, reason = "diagnostic surface for future hot-reload work")]
pub(crate) struct LoadedModuleInfo {
	pub path: PathBuf,
	pub module_id: ModuleId,
	pub metadata: Arc<PluginMetadata>,
}

/// Scan `wasm_dir` for `*.wasm`, instantiate the wasm runtime on
/// first successful load, register every export, and return the
/// bundle. Returns `None` when the directory is missing, empty, or
/// every load failed — the daemon then runs without a wasm runtime.
#[allow(clippy::too_many_lines, reason = "linear boot phase: scan + load + policy parse")]
pub(crate) async fn load_all(wasm_dir: &Path) -> Option<LoadedWasm> {
	let entries = match std::fs::read_dir(wasm_dir) {
		Ok(rd) => rd,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			tracing::info!(
				wasm_dir = %wasm_dir.display(),
				"wasm dir not present; skipping wasm runtime",
			);
			return None;
		}
		Err(e) => {
			tracing::warn!(
				wasm_dir = %wasm_dir.display(),
				error = %e,
				"failed to read wasm dir; skipping wasm runtime",
			);
			return None;
		}
	};

	let mut wasm_files: Vec<PathBuf> = entries
		.filter_map(Result::ok)
		.map(|e| e.path())
		.filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "wasm"))
		.collect();
	wasm_files.sort();

	if wasm_files.is_empty() {
		tracing::info!(
			wasm_dir = %wasm_dir.display(),
			"no .wasm modules found; skipping wasm runtime",
		);
		return None;
	}

	let backend: Arc<dyn HttpFetchBackend> = match HyperHttpFetchBackend::new() {
		Ok(b) => Arc::new(b),
		Err(e) => {
			tracing::warn!(
				error = %e,
				"hyper http-fetch backend construction failed; skipping wasm runtime",
			);
			return None;
		}
	};
	let runtime = match WasmtimeRuntime::new(backend) {
		Ok(rt) => rt,
		Err(e) => {
			tracing::warn!(error = %e, "wasm runtime construction failed; skipping wasm runtime");
			return None;
		}
	};

	let mut registry = PluginRegistry::new();
	let mut modules = Vec::new();
	let mut registered_exports = 0usize;

	for path in &wasm_files {
		let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
			tracing::warn!(
				path = %path.display(),
				"wasm module path has no UTF-8 file stem; skipping",
			);
			continue;
		};
		let metadata = match runtime.load_component(path).await {
			Ok(meta) => meta,
			Err(e) => {
				tracing::warn!(
					path = %path.display(),
					error = %e,
					"wasm module load failed; skipping",
				);
				continue;
			}
		};
		let module_id = ModuleId(Arc::from(path.to_string_lossy().as_ref()));
		let runtime_for_registry: Arc<dyn vane_core::WasmRuntime> = Arc::clone(&runtime) as _;
		for export in &metadata.exports {
			let plugin_name = format!("{stem}:{}", export.name);
			registry.register(
				&plugin_name,
				module_id.clone(),
				export.name.clone(),
				Arc::clone(&metadata),
				Arc::clone(&runtime_for_registry),
			);
			registered_exports += 1;
		}
		tracing::info!(
			path = %path.display(),
			plugin = %metadata.name,
			version = %metadata.version,
			exports = metadata.exports.len(),
			"loaded wasm module",
		);
		modules.push(LoadedModuleInfo { path: path.clone(), module_id, metadata });
	}

	if registered_exports == 0 {
		tracing::warn!(
			wasm_dir = %wasm_dir.display(),
			scanned = wasm_files.len(),
			"every wasm module failed to load; dropping runtime",
		);
		return None;
	}

	let policies = match PluginPolicyTable::load_from_dir(wasm_dir) {
		Ok(t) => {
			if t.policies.is_empty() {
				tracing::warn!(
					wasm_dir = %wasm_dir.display(),
					"no wasm/policy.json present; every plugin starts under deny-all http-fetch \
					 (allowed_hosts = []). Add policy.json to grant outbound access.",
				);
			} else {
				tracing::info!(policies = t.policies.len(), "loaded plugin http policy table",);
			}
			t
		}
		Err(e) => {
			tracing::warn!(
				wasm_dir = %wasm_dir.display(),
				error = %e,
				"wasm/policy.json failed to parse; falling back to deny-all defaults",
			);
			PluginPolicyTable::new()
		}
	};

	// Apply each module's resolved policy onto the runtime so
	// invoke_* calls see the operator-owned view at host-fn time.
	for module in &modules {
		let stem = module.path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
		let policy = Arc::new(policies.get_or_default(stem));
		runtime.set_policy(&module.module_id, policy);
	}

	Some(LoadedWasm { runtime, registry: Arc::new(registry), policies: Arc::new(policies), modules })
}

#[cfg(test)]
mod tests {
	use std::fs;

	use super::*;

	#[tokio::test]
	async fn load_all_returns_none_when_dir_missing() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let absent = tmp.path().join("does-not-exist");
		assert!(load_all(&absent).await.is_none());
	}

	#[tokio::test]
	async fn load_all_returns_none_when_dir_empty() {
		let tmp = tempfile::tempdir().expect("tempdir");
		assert!(load_all(tmp.path()).await.is_none());
	}

	#[tokio::test]
	async fn load_all_returns_none_when_only_non_wasm_files() {
		let tmp = tempfile::tempdir().expect("tempdir");
		fs::write(tmp.path().join("readme.md"), b"not wasm").unwrap();
		fs::write(tmp.path().join("garbage"), b"definitely not wasm").unwrap();
		assert!(load_all(tmp.path()).await.is_none());
	}

	#[tokio::test]
	async fn load_all_skips_unreadable_modules_and_returns_none_when_all_fail() {
		let tmp = tempfile::tempdir().expect("tempdir");
		fs::write(tmp.path().join("broken.wasm"), b"not a real component").unwrap();
		// Single broken file → no successful load → no runtime.
		assert!(load_all(tmp.path()).await.is_none());
	}

	#[tokio::test]
	async fn load_all_registers_exports_with_file_stem_prefix() {
		// Reuse the metadata fixture built by `vane-wasm`'s build.rs.
		let fixture_src =
			concat!(env!("CARGO_MANIFEST_DIR"), "/../wasm/fixtures/metadata_fixture.wasm");
		let fixture_src = std::path::Path::new(fixture_src);
		assert!(fixture_src.exists(), "wasm fixture not built yet");

		let tmp = tempfile::tempdir().expect("tempdir");
		// Copy under a known stem so the registered plugin name is
		// deterministic ("plugin_a" instead of build.rs's basename).
		let target = tmp.path().join("plugin_a.wasm");
		fs::copy(fixture_src, &target).expect("copy fixture");

		let loaded = load_all(tmp.path()).await.expect("loader returns Some");
		assert_eq!(loaded.modules.len(), 1);
		// Fixture exports `probe` of kind L4Peek; reference name is
		// `<stem>:<export>` per spec § Module lifecycle.
		assert!(
			loaded.registry.get("plugin_a:probe").is_some(),
			"registry must key by `<stem>:<export>`",
		);
		// Pool snapshot should be empty until rules instantiate pools.
		assert!(
			vane_core::WasmPoolStats::snapshot(&*loaded.runtime).is_empty(),
			"no rules linked yet; pool snapshot must be empty",
		);
	}
}
