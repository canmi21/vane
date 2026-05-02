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
