//! Foundation types, traits, `FlowGraph` IR, and compilation pipeline for vane.
//!
//! See `spec/architecture/03-types.md`, `02-flow.md`, `04-middleware.md`.

pub mod meta {
	pub const LICENSE: &str = "MIT";
	pub const REPOSITORY: &str = "https://github.com/canmi21/vane";
	pub const COPYRIGHT: &str = "Copyright © 2025 Canmi (t@canmi.icu)";
}

pub mod version {
	use std::fmt::Write as _;

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
	#[must_use]
	pub fn format_version(info: &BuildInfo) -> String {
		let mut out = String::new();
		let _ = writeln!(out, "vane {} ({} {})", info.version, info.commit, info.build_date);
		let _ = writeln!(out, "rustc {}", info.rustc);
		let _ = writeln!(out, "cargo {}", info.cargo);
		if !info.features.is_empty() {
			let _ = writeln!(out, "features:  {}", info.features.join(", "));
		}
		if !info.protocols.is_empty() {
			let _ = writeln!(out, "protocols: {}", info.protocols.join(", "));
		}
		out.push('\n');
		let _ = writeln!(out, "{}", super::meta::COPYRIGHT);
		let _ = writeln!(out, "License: {}", super::meta::LICENSE);
		let _ = writeln!(out, "Repository: {}", super::meta::REPOSITORY);
		out
	}
}
