//! Wasm component fixture paths.
//!
//! Both fixtures are built by [`build.rs`](../../build.rs) under
//! the `wasm-fixtures` cargo feature; the absolute paths into
//! `OUT_DIR` are baked in at testutil compile time and exposed to
//! consumers via these accessor functions. See `build.rs` for the
//! WIT/WAT inputs that drive generation.

use std::path::Path;

/// Path to the full plugin fixture (exports `registry` +
/// `handler-l4-peek`; metadata claims the `probe`/`l4-peek`
/// export). Used for the happy-path load + metadata smoke test.
#[must_use]
pub fn metadata() -> &'static Path {
	Path::new(env!("VANE_TESTUTIL_WASM_METADATA_FIXTURE"))
}

/// Path to the kind-mismatch fixture (exports `registry` only,
/// metadata still advertises an `l4-peek` export). Used to verify
/// that `load_component` rejects component-vs-metadata
/// inconsistencies.
#[must_use]
pub fn mismatch() -> &'static Path {
	Path::new(env!("VANE_TESTUTIL_WASM_MISMATCH_FIXTURE"))
}
