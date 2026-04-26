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

use vane_core::{
	Error, FetchKind, FetchMetadata, FetchMetadataProvider, FetchOutputModes, FetchPhase,
	MiddlewareKind, MiddlewareMetadata, MiddlewareMetadataProvider,
};

pub(crate) struct MetadataProviders;

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
