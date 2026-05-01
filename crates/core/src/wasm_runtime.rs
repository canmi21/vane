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

/// Runtime contract between the executor and the WASM plugin layer.
///
/// Declared in `vane-core`; the concrete implementation lives in
/// `vane-wasm` (`WasmtimeRuntime`). `vaned` constructs an
/// `Arc<dyn WasmRuntime>` at startup and injects it into the engine
/// before the first `FlowGraph` compilation that references WASM plugins.
///
/// At this stage the trait surface only covers component loading and
/// metadata retrieval. Invocation (`invoke`) is a later addition.
#[async_trait]
pub trait WasmRuntime: Send + Sync {
	/// Load a WASM component from disk, call `registry.get-metadata()`,
	/// validate the result, and return the cached metadata.
	///
	/// The runtime may consult a `.cwasm` content-hash cache to skip
	/// recompilation. Cache write failures are non-fatal (WARN log).
	async fn load_component(&self, path: &Path) -> Result<Arc<PluginMetadata>, Error>;
}
