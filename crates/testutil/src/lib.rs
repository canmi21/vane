//! Test helpers shared across integration tests. Dev-only, never linked into release.
//!
//! See [`spec/conventions.md` § _Testing_](../../../spec/conventions.md#testing).
//!
//! `unsafe_code` is allowed because the env-mutation helpers below
//! deliberately call `std::env::set_var`, which is `unsafe` under
//! Rust's 2024 edition. They run exactly once via `std::sync::Once`
//! before any TLS code touches the env, so the documented race
//! window cannot occur.

#![allow(unsafe_code)]

#[cfg(feature = "acme")]
pub mod acme;
pub mod echo;
pub mod flow;
#[cfg(feature = "h3")]
pub mod h3;
#[cfg(feature = "ocsp")]
pub use ocsp_mock_responder as ocsp;
pub mod port;
pub mod tracing;
pub mod vaned_fixture;
#[cfg(feature = "wasm-fixtures")]
pub mod wasm_fixture;

/// Opt into the per-upstream `tls.insecure_skip_verify: true` knob
/// for the rest of the test binary. Tests that bring up a self-
/// signed TLS upstream cannot pass real verification, so they need
/// the daemon's env-level master switch flipped on; production
/// binaries leave `VANE_ALLOW_INSECURE_UPSTREAM` unset and the
/// parser rejects the config.
///
/// Idempotent — wraps a `std::sync::Once`. Safe to call from every
/// test that sets `insecure_skip_verify: true`; first call wins.
pub fn allow_insecure_upstream_for_tests() {
	use std::sync::Once;
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		// SAFETY: env mutation is racy with concurrent reads, but
		// the `Once` barrier guarantees this runs before any test
		// body fires the parser. Within one binary the env var is
		// set exactly once.
		unsafe {
			std::env::set_var("VANE_ALLOW_INSECURE_UPSTREAM", "1");
		}
	});
}
