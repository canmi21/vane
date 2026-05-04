use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Error;
use crate::middleware::MiddlewareKind;

/// Metadata for a single exported middleware within a WASM component.
///
/// Populated from `registry.get-metadata()` at component load time.
#[derive(Debug, Clone)]
pub struct PluginExport {
	pub name: String,
	pub kind: MiddlewareKind,
	pub stateless: bool,
	pub needs_body: bool,
	pub inspects: Vec<String>,
}

/// Cached result of `registry.get-metadata()` for one WASM component.
#[derive(Debug)]
pub struct PluginMetadata {
	pub name: String,
	pub version: String,
	pub abi_version: String,
	pub exports: Vec<PluginExport>,
}

/// Stable identity for a loaded WASM component.
///
/// Per `spec/wasm-abi.md` § _Module identity_: the canonical absolute
/// filesystem path of the `.wasm` file.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ModuleId(pub Arc<str>);

/// Mirrors the WIT `context-value` variant from `vane:plugin/types@0.1.0`.
pub enum ContextValue {
	Text(String),
	Bytes(Vec<u8>),
	Int64(i64),
	Uint64(u64),
	Boolean(bool),
	ListText(Vec<String>),
}

/// Mirrors the WIT `context-entry` record from `vane:plugin/types@0.1.0`.
pub struct ContextEntry {
	pub path: String,
	pub value: ContextValue,
}

/// Mirrors the WIT `header` record from `vane:plugin/types@0.1.0`.
///
/// Names are guaranteed ASCII-lowercase by the host before being passed to plugins.
#[derive(Debug, Clone)]
pub struct Header {
	pub name: String,
	pub value: String,
}

/// Mirrors the WIT `bytes-view` record from `vane:plugin/types@0.1.0`.
#[derive(Debug, Clone)]
pub struct BytesView {
	pub data: Vec<u8>,
	pub truncated: bool,
}

/// Mirrors the WIT `l4-peek-input` record from `vane:plugin/handler-l4-peek@0.1.0`.
pub struct L4PeekInput {
	pub peek: Vec<u8>,
	pub context: Vec<ContextEntry>,
}

/// Mirrors the WIT `l4-peek-decision` variant from `vane:plugin/handler-l4-peek@0.1.0`.
#[derive(Debug)]
pub enum L4PeekDecision {
	Continue,
	Close,
}

/// Mirrors the WIT `l4-bytes-input` record from `vane:plugin/handler-l4-bytes@0.1.0`.
pub struct L4BytesInput {
	pub bytes: BytesView,
	pub context: Vec<ContextEntry>,
}

/// Mirrors the WIT `l4-bytes-decision` variant from `vane:plugin/handler-l4-bytes@0.1.0`.
#[derive(Debug)]
pub enum L4BytesDecision {
	Continue,
	Tunnel,
	Close,
}

/// Mirrors the WIT `l7-request-input` record from `vane:plugin/handler-l7-request@0.1.0`.
pub struct L7RequestInput {
	pub method: String,
	pub uri: String,
	pub headers: Vec<Header>,
	pub body: Option<BytesView>,
	pub context: Vec<ContextEntry>,
}

/// Mirrors the WIT `synth-response` record from `vane:plugin/handler-l7-request@0.1.0`.
#[derive(Debug, Clone)]
pub struct SynthResponse {
	pub status: u16,
	pub headers: Vec<Header>,
	pub body: Vec<u8>,
}

/// Mirrors the WIT `l7-request-decision` variant from `vane:plugin/handler-l7-request@0.1.0`.
#[derive(Debug)]
pub enum L7RequestDecision {
	Continue,
	Short(SynthResponse),
	Close,
}

/// Mirrors the WIT `l7-response-input` record from `vane:plugin/handler-l7-response@0.1.0`.
pub struct L7ResponseInput {
	pub status: u16,
	pub headers: Vec<Header>,
	pub body: Option<BytesView>,
	pub context: Vec<ContextEntry>,
}

/// Mirrors the WIT `modified-response` record from `vane:plugin/handler-l7-response@0.1.0`.
#[derive(Debug, Clone)]
pub struct ModifiedResponse {
	pub status: Option<u16>,
	pub headers: Option<Vec<Header>>,
	pub body: Option<Vec<u8>>,
}

/// Mirrors the WIT `l7-response-decision` variant from `vane:plugin/handler-l7-response@0.1.0`.
#[derive(Debug)]
pub enum L7ResponseDecision {
	Continue,
	Modify(ModifiedResponse),
	Abort,
}

/// Structured error from a plugin invocation.
///
/// `Plugin` wraps an in-band WIT error. `Trap` indicates a guest trap or
/// epoch timeout. `Exhausted` means all pooled instances are checked out.
#[derive(Debug)]
pub enum PluginError {
	Plugin { code: String, message: String, on_error_hint: Option<String> },
	Trap(String),
	Exhausted,
}

/// Runtime contract between the executor and the WASM plugin layer.
///
/// Declared in `vane-core`; the concrete implementation lives in
/// `vane-wasm` (`WasmtimeRuntime`). `vaned` constructs an
/// `Arc<dyn WasmRuntime>` at startup and injects it into the engine
/// before the first `FlowGraph` compilation that references WASM plugins.
#[async_trait]
pub trait WasmRuntime: Send + Sync {
	/// Load a WASM component from disk, call `registry.get-metadata()`,
	/// validate the result, and return the cached metadata.
	///
	/// The runtime may consult a `.cwasm` content-hash cache to skip
	/// recompilation. Cache write failures are non-fatal (WARN log).
	async fn load_component(&self, path: &Path) -> Result<Arc<PluginMetadata>, Error>;

	/// Invoke the `l4-peek` handler exported by the named component.
	///
	/// `module_id` must previously have been loaded via `load_component`.
	/// `export_name` selects which middleware export to call. `args_json`
	/// is the per-call-site configuration string delivered to the plugin
	/// via `host.get-args`. `input` carries the peek buffer and context.
	///
	/// Returns `PluginError::Trap` if the component has not been loaded.
	async fn invoke_l4_peek(
		&self,
		module_id: &ModuleId,
		export_name: &str,
		args_json: &str,
		input: L4PeekInput,
	) -> Result<L4PeekDecision, PluginError>;

	/// Invoke the `l4-bytes` handler exported by the named component.
	async fn invoke_l4_bytes(
		&self,
		module_id: &ModuleId,
		export_name: &str,
		args_json: &str,
		input: L4BytesInput,
	) -> Result<L4BytesDecision, PluginError>;

	/// Invoke the `l7-request` handler exported by the named component.
	async fn invoke_l7_request(
		&self,
		module_id: &ModuleId,
		export_name: &str,
		args_json: &str,
		input: L7RequestInput,
	) -> Result<L7RequestDecision, PluginError>;

	/// Invoke the `l7-response` handler exported by the named component.
	async fn invoke_l7_response(
		&self,
		module_id: &ModuleId,
		export_name: &str,
		args_json: &str,
		input: L7ResponseInput,
	) -> Result<L7ResponseDecision, PluginError>;
}

/// One pool entry surfaced by [`WasmPoolStats::snapshot`]. Mirrors the
/// shape `vane-wasm` produces internally; lives in `vane-core` so the
/// daemon can consume the data via a trait object without depending on
/// `vane-wasm` (which sits behind the optional `wasm` feature).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmPoolSummary {
	/// `"stateful"` or `"stateless"`. Static-string in `vane-wasm`,
	/// owned here so the trait object can return any backend's labels.
	pub kind: String,
	/// Module identity — typically the canonical absolute path of the
	/// `.wasm` file (matches [`ModuleId`]).
	pub key: String,
	/// Export name within the component (e.g. `"l4-peek"`).
	pub export: String,
	/// Pre-warmed instance count for the pool. `0` when the pool has
	/// no warm cache (e.g. on-demand stateless instantiation).
	pub capacity: usize,
	/// Currently checked-in instances. `capacity - available` is the
	/// number in flight; the daemon translates that to `in_use` on the
	/// wire.
	pub available: usize,
}

/// Read-only introspection of WASM pool runtime state. Implemented by
/// `vane-wasm::WasmtimeRuntime`; held by the daemon as
/// `Option<Arc<dyn WasmPoolStats>>` so builds without the optional
/// `wasm` feature can still consume the trait surface and serve the
/// `get_pools` mgmt verb (returning an empty list).
pub trait WasmPoolStats: Send + Sync {
	/// Snapshot every live pool. Read-only: must not instantiate
	/// modules, build instances, or mutate runtime state. Returning
	/// stale entries is acceptable — implementations may prune dead
	/// weak refs as part of the snapshot.
	fn snapshot(&self) -> Vec<WasmPoolSummary>;
}

/// Operator-owned per-plugin policy gating outbound `http-fetch`
/// calls and bounding their body / timeout / redirect behaviour.
///
/// Plugin authors do not declare these fields — the WIT metadata
/// only describes the plugin's exports. The daemon reads
/// `<config_dir>/wasm/policy.json` (top-level keys = `.wasm` file
/// stem) and constructs one [`PluginHttpPolicy`] per loaded module.
/// Plugins missing from the config file get [`PluginHttpPolicy::default`]
/// — an explicit deny-all posture (`allowed_hosts` empty) so an
/// operator who hasn't reviewed a plugin's network surface can't
/// be surprised by it reaching out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct PluginHttpPolicy {
	/// When `false` (default), `http-fetch` requests with
	/// `verify_tls: false` short-circuit to `InsecureRejected` regardless
	/// of the per-call value.
	#[serde(default)]
	pub allow_insecure: bool,
	/// Allowed-host pattern list. Each entry is either a literal
	/// hostname (`"api.internal"`), a wildcard prefix
	/// (`"*.example.com"` matches `a.example.com` / `b.c.example.com`
	/// but not `example.com`), or the universal wildcard `"*"`. An
	/// empty list (the default) is deny-all.
	#[serde(default)]
	pub allowed_hosts: Vec<String>,
	/// Per-request body cap (bytes) for the response body and the
	/// outbound request body. Default 1 MiB matches
	/// [`HttpFetchLimits::default`]'s `max_body_bytes`.
	#[serde(default = "default_max_body_size")]
	pub max_body_size: u32,
	/// Default timeout when the per-call `timeout_ms` is `None`.
	/// Default 30 s matches the spec's daemon default.
	#[serde(default = "default_timeout_ms")]
	pub default_timeout_ms: u32,
	/// Default redirect follow cap when the per-call
	/// `follow_redirects` is `None`. `0` disables redirects. Default 5.
	#[serde(default = "default_follow_redirects")]
	pub default_follow_redirects: u32,
}

const fn default_max_body_size() -> u32 {
	1024 * 1024
}

const fn default_timeout_ms() -> u32 {
	30_000
}

const fn default_follow_redirects() -> u32 {
	5
}

impl Default for PluginHttpPolicy {
	fn default() -> Self {
		Self {
			allow_insecure: false,
			allowed_hosts: Vec::new(),
			max_body_size: default_max_body_size(),
			default_timeout_ms: default_timeout_ms(),
			default_follow_redirects: default_follow_redirects(),
		}
	}
}

/// Operator-owned policy table keyed by `.wasm` file stem. Built at
/// boot from `<config_dir>/wasm/policy.json` and looked up by the
/// daemon's wasm loader when constructing per-plugin host state.
#[derive(Debug, Clone, Default)]
pub struct PluginPolicyTable {
	pub policies: std::collections::HashMap<String, PluginHttpPolicy>,
}

impl PluginPolicyTable {
	#[must_use]
	pub fn new() -> Self {
		Self { policies: std::collections::HashMap::new() }
	}

	/// Parse a `policy.json` whose top-level shape is
	/// `{ "<stem>": { ...PluginHttpPolicy fields... } }`. Missing
	/// fields per entry resolve to [`PluginHttpPolicy::default`]
	/// values via serde defaults.
	///
	/// # Errors
	/// Returns [`Error::compile`] when the JSON is malformed or any
	/// entry fails to deserialize as [`PluginHttpPolicy`].
	pub fn from_json(s: &str) -> Result<Self, Error> {
		let policies: std::collections::HashMap<String, PluginHttpPolicy> =
			serde_json::from_str(s).map_err(|e| Error::compile(format!("wasm/policy.json: {e}")))?;
		Ok(Self { policies })
	}

	/// Load `<wasm_dir>/policy.json` into a [`PluginPolicyTable`].
	/// Returns [`PluginPolicyTable::default`] (empty table) when the
	/// file is absent. Surfaces parse errors verbatim.
	///
	/// # Errors
	/// Returns [`Error::compile`] when the file exists but cannot be
	/// read or parsed.
	pub fn load_from_dir(wasm_dir: &std::path::Path) -> Result<Self, Error> {
		let path = wasm_dir.join("policy.json");
		match std::fs::read_to_string(&path) {
			Ok(s) => Self::from_json(&s),
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
			Err(e) => Err(Error::compile(format!("wasm/policy.json: read {}: {e}", path.display()))),
		}
	}

	/// Get the policy for a plugin by file stem, or
	/// [`PluginHttpPolicy::default`] when absent.
	#[must_use]
	pub fn get_or_default(&self, stem: &str) -> PluginHttpPolicy {
		self.policies.get(stem).cloned().unwrap_or_default()
	}
}

#[cfg(test)]
mod policy_tests {
	use super::*;

	#[test]
	fn default_policy_is_deny_all() {
		let p = PluginHttpPolicy::default();
		assert!(!p.allow_insecure);
		assert!(p.allowed_hosts.is_empty(), "deny-all by default");
		assert_eq!(p.max_body_size, 1024 * 1024);
		assert_eq!(p.default_timeout_ms, 30_000);
		assert_eq!(p.default_follow_redirects, 5);
	}

	#[test]
	fn policy_table_round_trips_explicit_fields() {
		let json = r#"{
			"edge": {
				"allow_insecure": true,
				"allowed_hosts": ["api.internal", "*.example.com"],
				"max_body_size": 65536,
				"default_timeout_ms": 5000,
				"default_follow_redirects": 0
			}
		}"#;
		let t = PluginPolicyTable::from_json(json).expect("parse");
		let p = t.get_or_default("edge");
		assert!(p.allow_insecure);
		assert_eq!(p.allowed_hosts, vec!["api.internal".to_string(), "*.example.com".to_string()]);
		assert_eq!(p.max_body_size, 65_536);
		assert_eq!(p.default_timeout_ms, 5000);
		assert_eq!(p.default_follow_redirects, 0);
	}

	#[test]
	fn policy_table_partial_entry_fills_defaults() {
		let json = r#"{ "edge": { "allowed_hosts": ["x.y"] } }"#;
		let t = PluginPolicyTable::from_json(json).expect("parse");
		let p = t.get_or_default("edge");
		assert_eq!(p.allowed_hosts, vec!["x.y".to_string()]);
		assert_eq!(p.max_body_size, 1024 * 1024, "default fills");
		assert_eq!(p.default_timeout_ms, 30_000);
	}

	#[test]
	fn policy_table_missing_plugin_returns_deny_all_default() {
		let t = PluginPolicyTable::from_json(r#"{ "other": {} }"#).expect("parse");
		let p = t.get_or_default("missing");
		assert_eq!(p, PluginHttpPolicy::default());
	}

	#[test]
	fn policy_table_load_from_dir_handles_absent_file() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let t = PluginPolicyTable::load_from_dir(tmp.path()).expect("absent ok");
		assert!(t.policies.is_empty());
	}

	#[test]
	fn policy_table_load_from_dir_parses_json() {
		let tmp = tempfile::tempdir().expect("tempdir");
		std::fs::write(tmp.path().join("policy.json"), r#"{ "x": { "allowed_hosts": ["*"] } }"#)
			.expect("write");
		let t = PluginPolicyTable::load_from_dir(tmp.path()).expect("parse");
		assert_eq!(t.get_or_default("x").allowed_hosts, vec!["*".to_string()]);
	}

	#[test]
	fn policy_table_load_from_dir_propagates_parse_errors() {
		let tmp = tempfile::tempdir().expect("tempdir");
		std::fs::write(tmp.path().join("policy.json"), "{ this is not json").expect("write");
		let err = PluginPolicyTable::load_from_dir(tmp.path()).expect_err("must fail");
		assert!(err.to_string().contains("policy.json"));
	}
}
