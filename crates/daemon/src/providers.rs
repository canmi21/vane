//! Daemon-side metadata provider for the core compile pipeline.
//!
//! Lists exactly the middleware / fetch shapes that the daemon registers
//! with engine factories — `host_header_match`, `path_prefix`,
//! `method_match`, `forward_client_ip`, plus `HttpProxy`,
//! `HttpSynthesize`, `L4Forward`. Compile and link agree on the
//! registered set: anything else fails compile with `unknown
//! middleware` / `unknown fetch`, or fails link with `UnknownFetch` /
//! `UnknownMiddleware` if the metadata provider is permissive.
//!
//! Lives in its own module so both `main.rs` boot path and `reload.rs`
//! recompile path can share one source of truth.

#[cfg(feature = "wasm")]
use std::sync::Arc;

use vane_core::{
	Error, FetchKind, FetchMetadata, FetchMetadataProvider, FetchOutputModes, FetchPhase,
	MiddlewareKind, MiddlewareMetadata, MiddlewareMetadataProvider,
};
#[cfg(feature = "wasm")]
use vane_engine::flow_graph::PluginRegistry;

#[derive(Default)]
pub(crate) struct MetadataProviders {
	/// Optional plugin registry consulted for `<module>:<export>`
	/// (colon-bearing) middleware names. `None` when the daemon was
	/// built without the `wasm` feature, or when the boot scan
	/// produced no live plugins. Stored as `Arc` so a single registry
	/// is shared across boot, reload, and `compile_dry_run`.
	#[cfg(feature = "wasm")]
	pub plugin_registry: Option<Arc<PluginRegistry>>,
}

impl MetadataProviders {
	#[cfg(feature = "wasm")]
	#[allow(dead_code, reason = "alternate constructor for tests + non-wasm code paths")]
	pub(crate) fn new() -> Self {
		Self { plugin_registry: None }
	}

	#[cfg(not(feature = "wasm"))]
	#[allow(dead_code, reason = "alternate constructor for tests + non-wasm code paths")]
	pub(crate) fn new() -> Self {
		Self {}
	}

	#[cfg(feature = "wasm")]
	pub(crate) fn with_plugins(registry: Arc<PluginRegistry>) -> Self {
		Self { plugin_registry: Some(registry) }
	}
}

#[allow(clippy::unnecessary_wraps)]
fn validate_args_pass(_: &serde_json::Value) -> Result<(), Error> {
	// Per-factory args validation lives inside each factory at link
	// time. The compile pipeline only needs `Some(meta)` to confirm the
	// name is registered — schema violations surface as `LinkError`
	// later via the engine factory's args-parse path.
	Ok(())
}

impl MiddlewareMetadataProvider for MetadataProviders {
	fn get(&self, name: &str) -> Option<MiddlewareMetadata> {
		// Colon-bearing names are plugin references (`<module>:<export>`).
		// Native middleware names are pure ASCII identifiers and never
		// contain a colon, so the split is unambiguous.
		if name.contains(':') {
			return self.lookup_plugin(name);
		}
		let (kind, stateless, needs_body) = match name {
			"host_header_match" | "path_prefix" | "method_match" | "forward_client_ip" => {
				(MiddlewareKind::L7Request, true, false)
			}
			// `rate_limit` is the canonical stateful middleware — per
			// spec/architecture/04-middleware.md § _Stateful internal_,
			// `stateless: false` so `lower::intern_middleware` skips
			// dedup and every call site gets its own bucket.
			"rate_limit" => (MiddlewareKind::L7Request, false, false),
			_ => return None,
		};
		Some(MiddlewareMetadata { kind, stateless, needs_body, validate_args: validate_args_pass })
	}
}

impl MetadataProviders {
	#[cfg(feature = "wasm")]
	fn lookup_plugin(&self, name: &str) -> Option<MiddlewareMetadata> {
		let registry = self.plugin_registry.as_ref()?;
		let entry = registry.get(name)?;
		// The registered export must exist on the cached metadata; if it
		// doesn't the registry is internally inconsistent and we treat
		// the name as unknown so the compile pipeline surfaces a clean
		// error.
		let export = entry.metadata.exports.iter().find(|e| e.name == entry.export_name)?;
		Some(MiddlewareMetadata::from_plugin(export))
	}

	#[cfg(not(feature = "wasm"))]
	#[allow(clippy::unused_self)]
	fn lookup_plugin(&self, _name: &str) -> Option<MiddlewareMetadata> {
		None
	}
}

impl FetchMetadataProvider for MetadataProviders {
	fn get(&self, kind: FetchKind) -> Option<FetchMetadata> {
		let (phase, output_modes) = match kind {
			FetchKind::L4Forward => (FetchPhase::L4, FetchOutputModes { response: false, tunnel: true }),
			FetchKind::HttpProxy | FetchKind::HttpSynthesize => {
				(FetchPhase::L7, FetchOutputModes { response: true, tunnel: false })
			}
			FetchKind::WebSocketUpgrade => {
				(FetchPhase::L7, FetchOutputModes { response: true, tunnel: true })
			}
		};
		Some(FetchMetadata { kind, phase, output_modes, validate_args: validate_args_pass })
	}
}
