//! Foundation types, traits, `FlowGraph` IR, and compilation pipeline for vane.
//!
//! See `spec/architecture/03-types.md`, `02-flow.md`, `04-middleware.md`.

pub mod body;
pub use body::*;
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
pub use predicate::*;
pub mod protocol_detect;
pub use protocol_detect::*;
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
	use std::fmt::Write as _;

	use super::meta::{COPYRIGHT, DESCRIPTION, HOMEPAGE, LICENSE_URL, REPOSITORY};

	/// Compile-time and runtime information about a vane binary.
	///
	/// Constructed by each binary from its own `build.rs`-emitted env vars
	/// and `cfg!(feature = ...)` introspection. See
	/// `spec/architecture/16-crate-layout.md`.
	pub struct BuildInfo {
		pub version: &'static str,
		pub commit: &'static str,
		pub build_date: &'static str,
		pub rustc: &'static str,
		pub cargo: &'static str,
		pub features: &'static [&'static str],
		pub protocols: &'static [&'static str],
	}

	/// Format the shared version banner used by both `vane` and `vaned`.
	///
	/// Every content line is indented with two spaces; the output is bracketed
	/// by a leading and trailing blank line for vertical breathing room in the
	/// terminal.
	///
	/// Layout:
	/// ```text
	///
	///   Vane — A compact programmable proxy engine
	///
	///   Built:      <version> (<commit> <date>)
	///   Rust:       <rustc-version-line>
	///   Cargo:      <cargo-version-line>
	///   Features:   ...                              (vaned only)
	///   Protocols:  ...                              (vaned only)
	///
	///   Copyright (C) 2025 Canmi <t@canmi.icu>
	///
	///   Released under the MIT License without restriction.
	///   This software comes with ABSOLUTELY NO WARRANTY.
	///
	///   Homepage:   https://vane.canmi.app
	///   Source:     https://github.com/canmi21/vane
	///   License:    https://opensource.org/licenses/MIT
	///
	/// ```
	#[must_use]
	pub fn format_version(info: &BuildInfo) -> String {
		const WIDTH: usize = 12;
		const INDENT: &str = "  ";
		let mut out = String::new();

		let _ = writeln!(out);
		let _ = writeln!(out, "{INDENT}Vane — {DESCRIPTION}");
		let _ = writeln!(out);

		let _ = writeln!(
			out,
			"{INDENT}{label:<WIDTH$}{version} ({commit} {date})",
			label = "Built:",
			version = info.version,
			commit = info.commit,
			date = info.build_date,
		);
		let _ = writeln!(out, "{INDENT}{label:<WIDTH$}{value}", label = "Rust:", value = info.rustc);
		let _ = writeln!(out, "{INDENT}{label:<WIDTH$}{value}", label = "Cargo:", value = info.cargo);
		if !info.features.is_empty() {
			let _ = writeln!(
				out,
				"{INDENT}{label:<WIDTH$}{value}",
				label = "Features:",
				value = info.features.join(", "),
			);
		}
		if !info.protocols.is_empty() {
			let _ = writeln!(
				out,
				"{INDENT}{label:<WIDTH$}{value}",
				label = "Protocols:",
				value = info.protocols.join(", "),
			);
		}
		let _ = writeln!(out);
		let _ = writeln!(out, "{INDENT}{COPYRIGHT}");
		let _ = writeln!(out);
		let _ = writeln!(out, "{INDENT}Released under the MIT License without restriction.");
		let _ = writeln!(out, "{INDENT}This software comes with ABSOLUTELY NO WARRANTY.");
		let _ = writeln!(out);
		let _ = writeln!(out, "{INDENT}{label:<WIDTH$}{HOMEPAGE}", label = "Homepage:");
		let _ = writeln!(out, "{INDENT}{label:<WIDTH$}{REPOSITORY}", label = "Source:");
		let _ = writeln!(out, "{INDENT}{label:<WIDTH$}{LICENSE_URL}", label = "License:");
		let _ = writeln!(out);

		out
	}
}
