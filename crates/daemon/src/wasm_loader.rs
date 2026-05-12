//! Boot-time WASM module discovery, instantiation, and registry build.
//!
//! Per `spec/crates/engine-wasm.md` § _Module lifecycle_, the daemon
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
//! [`load_all`] runs at boot; [`reload_dir`] runs on every file-watcher
//! reload, reconciling the runtime + registry against the current
//! filesystem state.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use vane_core::{
	Error, HttpFetchBackend, ModuleId, PluginMetadata, PluginPolicyTable, WasmRuntime,
};
use vane_engine::flow_graph::PluginRegistry;
use vane_engine::wasm_fetch::HyperHttpFetchBackend;
use vane_wasm::{ReloadComponentOutcome, WasmtimeRuntime};

/// Outcome of [`load_all`] when at least one `.wasm` was loaded.
pub(crate) struct LoadedWasm {
	pub runtime: Arc<WasmtimeRuntime>,
	/// Plugin registry held inside `ArcSwap` so the reload pipeline
	/// publishes a fresh registry atomically while the read path
	/// (compile, link) reads a stable `Arc<PluginRegistry>` snapshot.
	pub registry: Arc<ArcSwap<PluginRegistry>>,
	/// Operator-owned per-plugin policy table loaded from
	/// `<wasm_dir>/policy.json`. Held in `ArcSwap` for the same
	/// reload-time atomic-swap reason as `registry`.
	pub policies: Arc<ArcSwap<PluginPolicyTable>>,
	#[allow(dead_code, reason = "diagnostic surface for future hot-reload work")]
	pub modules: Vec<LoadedModuleInfo>,
}

/// Outcome of [`reload_dir`]: a single bit telling the caller whether
/// the rule-side flow graph must be recompiled. Per-module byte-only
/// changes (`MetadataUnchanged`) swap the component in place without
/// graph churn; metadata-relevant changes / module add / module drop
/// flip this to `true`.
pub(crate) struct WasmReloadOutcome {
	pub schema_changed: bool,
}

#[allow(dead_code, reason = "diagnostic surface for future hot-reload work")]
pub(crate) struct LoadedModuleInfo {
	pub path: PathBuf,
	pub module_id: ModuleId,
	pub metadata: Arc<PluginMetadata>,
}

/// List `*.wasm` paths in `wasm_dir`, sorted. NotFound is treated as
/// an empty directory — callers distinguish "missing" from "empty"
/// downstream by checking whether the result is empty (cheap) without
/// the extra signal in the return type.
fn discover_wasm_files(wasm_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
	let entries = match std::fs::read_dir(wasm_dir) {
		Ok(rd) => rd,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(e) => return Err(e),
	};
	let mut files: Vec<PathBuf> = entries
		.filter_map(Result::ok)
		.map(|e| e.path())
		.filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "wasm"))
		.collect();
	files.sort();
	Ok(files)
}

/// Construct the daemon-wide WASM runtime: HTTP fetch backend + the
/// `WasmtimeRuntime`. Both failures are warn-logged and surface as
/// `None` so the daemon proceeds without WASM.
fn build_wasm_runtime() -> Option<Arc<WasmtimeRuntime>> {
	let backend: Arc<dyn HttpFetchBackend> = match HyperHttpFetchBackend::new() {
		Ok(b) => Arc::new(b),
		Err(e) => {
			tracing::warn!(error = %e, "hyper http-fetch backend construction failed; skipping wasm runtime");
			return None;
		}
	};
	match WasmtimeRuntime::new(backend) {
		Ok(rt) => Some(rt),
		Err(e) => {
			tracing::warn!(error = %e.tracing(), "wasm runtime construction failed; skipping wasm runtime");
			None
		}
	}
}

/// Register every export of one loaded module under
/// `<stem>:<export>` keys; returns the export count for the caller's
/// running total.
fn register_module_exports(
	registry: &mut PluginRegistry,
	stem: &str,
	module_id: &ModuleId,
	runtime: &Arc<WasmtimeRuntime>,
	metadata: &Arc<PluginMetadata>,
) -> usize {
	let runtime_for_registry: Arc<dyn WasmRuntime> = Arc::clone(runtime) as _;
	let mut count = 0;
	for export in &metadata.exports {
		let plugin_name = format!("{stem}:{}", export.name);
		registry.register(
			&plugin_name,
			module_id.clone(),
			export.name.clone(),
			Arc::clone(metadata),
			Arc::clone(&runtime_for_registry),
		);
		count += 1;
	}
	count
}

/// Apply each module's resolved policy onto the runtime so subsequent
/// `invoke_*` calls see the operator-owned view at host-fn time.
fn apply_per_module_policies(
	runtime: &WasmtimeRuntime,
	modules: &[LoadedModuleInfo],
	policies: &PluginPolicyTable,
) {
	for module in modules {
		let stem = module.path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
		let policy = Arc::new(policies.get_or_default(stem));
		runtime.set_policy(&module.module_id, policy);
	}
}

/// Boot-side policy load: warn when policy.json is absent / empty,
/// info-log policy count, fall back to deny-all on parse error.
fn load_policies_for_boot(wasm_dir: &Path) -> PluginPolicyTable {
	match PluginPolicyTable::load_from_dir(wasm_dir) {
		Ok(t) => {
			if t.policies.is_empty() {
				tracing::warn!(
					wasm_dir = %wasm_dir.display(),
					"no wasm/policy.json present; every plugin starts under deny-all http-fetch \
					 (allowed_hosts = []). Add policy.json to grant outbound access.",
				);
			} else {
				tracing::info!(policies = t.policies.len(), "loaded plugin http policy table");
			}
			t
		}
		Err(e) => {
			tracing::warn!(
				wasm_dir = %wasm_dir.display(),
				error = %e.tracing(),
				"wasm/policy.json failed to parse; falling back to deny-all defaults",
			);
			PluginPolicyTable::new()
		}
	}
}

/// Reload-side policy load: keep the prior table on parse failure so
/// the daemon doesn't accidentally widen / narrow access during a
/// transient `policy.json` error.
fn load_policies_for_reload(
	wasm_dir: &Path,
	policies_swap: &Arc<ArcSwap<PluginPolicyTable>>,
) -> PluginPolicyTable {
	match PluginPolicyTable::load_from_dir(wasm_dir) {
		Ok(t) => t,
		Err(e) => {
			tracing::warn!(error = %e.tracing(), "wasm/policy.json reload failed; keeping prior policy table");
			(*policies_swap.load_full()).clone()
		}
	}
}

/// Snapshot the current registry into a stem-keyed map so the reload
/// pass can detect added / removed stems by membership testing.
fn snapshot_stems_from_registry(current: &PluginRegistry) -> BTreeMap<String, Vec<String>> {
	let mut by_stem: BTreeMap<String, Vec<String>> = BTreeMap::new();
	for (key, _) in current.iter() {
		if let Some((stem, _)) = key.split_once(':') {
			by_stem.entry(stem.to_owned()).or_default().push(key.to_owned());
		}
	}
	by_stem
}

/// Acquire metadata for one module during reload: known stems take the
/// `reload_component` path (which may report MetadataChanged → schema
/// flip); unknown stems take the fresh `load_component` path. Per-
/// module load failures are warn-logged and surface as `None` so the
/// caller skips this module without aborting the reload.
///
/// Returns `Some((metadata, schema_bumped))` on success; `schema_bumped`
/// is `true` for added modules and metadata-incompatible reloads.
async fn acquire_metadata_for_reload(
	runtime: &Arc<WasmtimeRuntime>,
	current: &PluginRegistry,
	stem: &str,
	module_id: &ModuleId,
	path: &Path,
	known_to_registry: bool,
) -> Result<Option<(Arc<PluginMetadata>, bool)>, Error> {
	if known_to_registry {
		match runtime.reload_component(path).await {
			Ok(ReloadComponentOutcome::Unchanged) => {
				tracing::debug!(stem, "wasm module unchanged on disk; reusing entries");
				let meta = runtime
					.metadata_for_module(module_id)
					.ok_or_else(|| Error::middleware(format!("metadata missing post-reload for {stem}")))?;
				Ok(Some((meta, false)))
			}
			Ok(ReloadComponentOutcome::MetadataUnchanged) => {
				tracing::info!(stem, "wasm module bytes changed; metadata-compatible swap");
				let meta = runtime
					.metadata_for_module(module_id)
					.ok_or_else(|| Error::middleware(format!("metadata missing post-reload for {stem}")))?;
				Ok(Some((meta, false)))
			}
			Ok(ReloadComponentOutcome::MetadataChanged) => {
				tracing::info!(stem, "wasm module bytes changed; metadata-incompatible recompile");
				let meta = runtime
					.metadata_for_module(module_id)
					.ok_or_else(|| Error::middleware(format!("metadata missing post-reload for {stem}")))?;
				Ok(Some((meta, true)))
			}
			Err(e) => {
				tracing::warn!(
					path = %path.display(),
					error = %e.tracing(),
					"wasm reload failed; keeping prior registry entries for this stem",
				);
				let prior = current
					.iter()
					.find(|(_, v)| v.module_id == *module_id)
					.map(|(_, v)| Arc::clone(&v.metadata));
				Ok(prior.map(|p| (p, false)))
			}
		}
	} else {
		match runtime.load_component(path).await {
			Ok(meta) => {
				tracing::info!(stem, "wasm module added");
				Ok(Some((meta, true)))
			}
			Err(e) => {
				tracing::warn!(
					path = %path.display(),
					error = %e.tracing(),
					"wasm module load failed during reload; skipping",
				);
				Ok(None)
			}
		}
	}
}

/// Drop runtime state for stems that disappeared from disk; returns
/// `true` if any drop happened (the caller flips `schema_changed`).
fn drop_disappeared_stems(
	runtime: &WasmtimeRuntime,
	current_by_stem: &BTreeMap<String, Vec<String>>,
	on_disk_stems: &BTreeSet<String>,
	current: &PluginRegistry,
) -> bool {
	let mut any_dropped = false;
	for stem in current_by_stem.keys() {
		if on_disk_stems.contains(stem) {
			continue;
		}
		any_dropped = true;
		if let Some(entry) = current.iter().find(|(k, _)| k.starts_with(&format!("{stem}:"))) {
			let module_id = entry.1.module_id.clone();
			runtime.unload_module(&module_id);
			tracing::info!(stem, "wasm module removed; runtime state unloaded");
		}
	}
	any_dropped
}

/// Scan `wasm_dir` for `*.wasm`, instantiate the wasm runtime on
/// first successful load, register every export, and return the
/// bundle. Returns `None` when the directory is missing, empty, or
/// every load failed — the daemon then runs without a wasm runtime.
pub(crate) async fn load_all(wasm_dir: &Path) -> Option<LoadedWasm> {
	let wasm_files = match discover_wasm_files(wasm_dir) {
		Ok(f) => f,
		Err(e) => {
			tracing::warn!(
				wasm_dir = %wasm_dir.display(),
				error = %e,
				"failed to read wasm dir; skipping wasm runtime",
			);
			return None;
		}
	};
	if wasm_files.is_empty() {
		tracing::info!(
			wasm_dir = %wasm_dir.display(),
			"no .wasm modules found; skipping wasm runtime",
		);
		return None;
	}

	let runtime = build_wasm_runtime()?;

	let mut registry = PluginRegistry::new();
	let mut modules = Vec::new();
	let mut registered_exports = 0usize;

	for path in &wasm_files {
		let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
			tracing::warn!(path = %path.display(), "wasm module path has no UTF-8 file stem; skipping");
			continue;
		};
		let metadata = match runtime.load_component(path).await {
			Ok(meta) => meta,
			Err(e) => {
				tracing::warn!(path = %path.display(), error = %e.tracing(), "wasm module load failed; skipping");
				continue;
			}
		};
		let module_id = ModuleId(Arc::from(path.to_string_lossy().as_ref()));
		registered_exports +=
			register_module_exports(&mut registry, &stem, &module_id, &runtime, &metadata);
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

	let policies = load_policies_for_boot(wasm_dir);
	apply_per_module_policies(&runtime, &modules, &policies);

	Some(LoadedWasm {
		runtime,
		registry: Arc::new(ArcSwap::from_pointee(registry)),
		policies: Arc::new(ArcSwap::from_pointee(policies)),
		modules,
	})
}

/// Re-scan `wasm_dir`, reconcile against the runtime's currently-
/// loaded modules, and update `registry_swap` + `policies_swap`
/// atomically. Per-module outcome:
///
/// * Bytes unchanged (hash match): no-op, registry entries stay.
/// * Bytes changed, metadata-compatible: runtime swaps `Component`
///   and bumps the stateful pool generation; the registry's
///   `Arc<PluginMetadata>` for that stem is replaced (the
///   `metadata.name` / `version` label may have moved). No graph
///   recompile is required.
/// * Bytes changed, metadata-incompatible: runtime updates and the
///   stem's registry entries are rebuilt; `schema_changed` flips on.
/// * New file: register every export against a freshly-loaded
///   component; `schema_changed` flips on.
/// * Removed file: `runtime.unload_module` drops the runtime state;
///   the registry omits the stem; `schema_changed` flips on. Rules
///   referencing the dropped stem fail at the next `link` step,
///   which is the standard reload-failure path.
///
/// Errors from individual modules are surfaced via `tracing::warn` —
/// a single broken `.wasm` does not abort the whole reload.
///
/// # Errors
///
/// Returns the first fatal error: `wasm_dir` unreadable, or
/// `policy.json` re-parse failure that would otherwise leave the
/// daemon with stale policy state.
pub(crate) async fn reload_dir(
	wasm_dir: &Path,
	runtime: &Arc<WasmtimeRuntime>,
	registry_swap: &Arc<ArcSwap<PluginRegistry>>,
	policies_swap: &Arc<ArcSwap<PluginPolicyTable>>,
) -> Result<WasmReloadOutcome, Error> {
	let wasm_files = discover_wasm_files(wasm_dir)
		.map_err(|e| Error::middleware(format!("read wasm dir {}: {e}", wasm_dir.display())))?;

	let current = registry_swap.load();
	let current_by_stem = snapshot_stems_from_registry(&current);
	let on_disk_stems: BTreeSet<String> = wasm_files
		.iter()
		.filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
		.collect();

	// Build the new registry from scratch — entries we keep get
	// re-registered against the latest metadata Arc, entries we
	// drop simply don't get added.
	let mut new_registry = PluginRegistry::new();
	let mut modules_seen: Vec<LoadedModuleInfo> = Vec::new();
	let mut schema_changed = false;

	for path in &wasm_files {
		let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
			tracing::warn!(path = %path.display(), "wasm file has no UTF-8 stem; skipping");
			continue;
		};
		let module_id = ModuleId(Arc::from(path.to_string_lossy().as_ref()));
		let known = current_by_stem.contains_key(&stem);
		let Some((metadata, bumped)) =
			acquire_metadata_for_reload(runtime, &current, &stem, &module_id, path, known).await?
		else {
			continue;
		};
		schema_changed |= bumped;
		register_module_exports(&mut new_registry, &stem, &module_id, runtime, &metadata);
		modules_seen.push(LoadedModuleInfo { path: path.clone(), module_id, metadata });
	}

	if drop_disappeared_stems(runtime, &current_by_stem, &on_disk_stems, &current) {
		schema_changed = true;
	}

	let policies = load_policies_for_reload(wasm_dir, policies_swap);
	apply_per_module_policies(runtime, &modules_seen, &policies);

	registry_swap.store(Arc::new(new_registry));
	policies_swap.store(Arc::new(policies));
	Ok(WasmReloadOutcome { schema_changed })
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
		// Fixture is generated into vane-testutil's OUT_DIR by its
		// `wasm-fixtures` feature; the path is baked in at testutil
		// compile time so we don't have to reach across crates.
		let fixture_src = vane_testutil::wasm_fixture::metadata();
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
			loaded.registry.load().get("plugin_a:probe").is_some(),
			"registry must key by `<stem>:<export>`",
		);
		// Pool snapshot should be empty until rules instantiate pools.
		assert!(
			vane_core::WasmPoolStats::snapshot(&*loaded.runtime).is_empty(),
			"no rules linked yet; pool snapshot must be empty",
		);
	}
}
