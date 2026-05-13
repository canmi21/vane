//! Foundation types, traits, `FlowGraph` IR, and compilation pipeline for vane.
//!
//! See `spec/crates/core.md`, `spec/flow-model.md`, `spec/crates/engine.md`.

pub mod body;
pub use body::*;
pub mod canonical;
pub mod compile;
pub use compile::compile;
pub mod config;
pub use config::{Env, EnvReader, LoadedConfig, ProcessEnv, load, scan_rules_dir};
pub mod conn_context;
pub use conn_context::*;
pub mod error;
pub use error::*;
pub mod fetch;
pub use fetch::*;
pub mod flow_ctx;
pub use flow_ctx::*;
pub mod flow_log;
pub use flow_log::*;
pub mod ir;
pub use ir::*;
pub mod l4;
pub use l4::*;
pub mod metadata;
pub use metadata::*;
pub mod middleware;
pub use middleware::*;
pub mod wasm_runtime;
pub use wasm_runtime::*;
pub mod phase;
pub mod predicate;
pub use guess::{DetectedProtocol, MAX_PEEK_BYTES, PeekResult, TlsClientHello};
pub use predicate::*;
pub mod preset;
pub use preset::{PresetInvocation, RuleEntry, expand_invocation};
pub mod rule;

pub mod meta {
	pub const DESCRIPTION: &str = "A compact programmable proxy engine";
	pub const COPYRIGHT: &str = "Copyright (C) 2025 Canmi <t@canmi.icu>";
	pub const HOMEPAGE: &str = "https://vane.canmi.app";
	pub const REPOSITORY: &str = "https://github.com/canmi21/vane";
	pub const LICENSE: &str = "MIT";
	pub const LICENSE_URL: &str = "https://opensource.org/licenses/MIT";
}

pub mod version {
	/// Compile-time and runtime information about a vane binary.
	///
	/// Pure data — no presentation deps. Each binary fills this in
	/// from its own `build.rs`-emitted env vars and
	/// `cfg!(feature = ...)` introspection, then hands it to the
	/// `vane-banner` crate for printing. See `spec/crates/daemon.md`
	/// § _Crypto provider_ and the per-binary `build.rs` files for
	/// the full contract.
	pub struct BuildInfo {
		pub version: &'static str,
		pub commit: &'static str,
		pub build_date: &'static str,
		pub rustc: &'static str,
		pub cargo: &'static str,
		pub features: &'static [&'static str],
		pub protocols: &'static [&'static str],
	}
}
