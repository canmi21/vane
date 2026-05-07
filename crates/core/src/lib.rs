//! Foundation types, traits, `FlowGraph` IR, and compilation pipeline for vane.
//!
//! See `spec/crates/core.md`, `02-flow.md`, `04-middleware.md`.

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
	use owo_colors::{OwoColorize, Stream, Style};

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

	/// Print the shared build banner used by both `vane -v` and
	/// `vaned -v`. Goes straight to stdout. ANSI colour escapes are
	/// emitted only when stdout is detected as a TTY (via owo-colors'
	/// `Stream::Stdout` check), so `vane -v | cat` still produces flat
	/// ASCII.
	///
	/// Palette (kept consistent with `vane`'s clap help output):
	/// - **Vane** brand → yellow + bold
	/// - section labels (`Built:`, `Rust:`, `Homepage:` …) → cyan + bold
	/// - MIT-licence prose lead-in (the two lines that introduce the
	///   warranty disclaimer) → green, mirroring the placeholder
	///   colour from clap help so the disclaimer block reads as a
	///   single styled paragraph
	/// - `ABSOLUTELY NO WARRANTY` substring → red + bold
	/// - everything else (description, version values, copyright,
	///   URL values) → plain
	///
	/// Layout (uncoloured shape):
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
	pub fn print_banner(info: &BuildInfo) {
		const WIDTH: usize = 12;
		const INDENT: &str = "  ";

		let brand_bold = Style::new().yellow().bold();
		let brand = Style::new().yellow();
		let prose = Style::new().cyan();
		let email = Style::new().green();
		let warning = Style::new().red().bold();

		println!();
		println!(
			"{INDENT}{} — {DESCRIPTION}",
			"Vane".if_supports_color(Stream::Stdout, |t| t.style(brand_bold)),
		);
		println!();

		print_label(
			"Built:",
			&format!("{} ({} {})", info.version, info.commit, info.build_date),
			WIDTH,
			INDENT,
		);
		print_label("Rust:", info.rustc, WIDTH, INDENT);
		print_label("Cargo:", info.cargo, WIDTH, INDENT);
		if !info.features.is_empty() {
			print_label("Features:", &info.features.join(", "), WIDTH, INDENT);
		}
		if !info.protocols.is_empty() {
			print_label("Protocols:", &info.protocols.join(", "), WIDTH, INDENT);
		}

		println!();
		// Split COPYRIGHT into three styled spans:
		//   "Copyright"          → yellow (matches the brand tone, no bold)
		//   " (C) 2025 Canmi "   → cyan, currently — second pass may flip
		//                          this to plain depending on review
		//   "<t@canmi.icu>"      → green, mirrors the prose accent
		let (copyright_word, rest) = COPYRIGHT.split_at("Copyright".len());
		let (middle, email_addr) = match rest.find('<') {
			Some(i) => rest.split_at(i),
			None => (rest, ""),
		};
		println!(
			"{INDENT}{}{}{}",
			copyright_word.if_supports_color(Stream::Stdout, |t| t.style(brand)),
			middle.if_supports_color(Stream::Stdout, |t| t.style(prose)),
			email_addr.if_supports_color(Stream::Stdout, |t| t.style(email)),
		);
		println!();
		// Each licence line keeps prose plain and styles only the
		// noun phrase that carries the meaning — cyan for the licence
		// reference, red-bold for the warranty disclaimer.
		println!(
			"{INDENT}Released under the {} without restriction.",
			"MIT License".if_supports_color(Stream::Stdout, |t| t.style(prose)),
		);
		println!(
			"{INDENT}This software comes with {}.",
			"ABSOLUTELY NO WARRANTY".if_supports_color(Stream::Stdout, |t| t.style(warning)),
		);
		println!();

		print_label("Homepage:", HOMEPAGE, WIDTH, INDENT);
		print_label("Source:", REPOSITORY, WIDTH, INDENT);
		print_label("License:", LICENSE_URL, WIDTH, INDENT);
		println!();
	}

	fn print_label(label: &str, value: &str, width: usize, indent: &str) {
		let label_style = Style::new().cyan().bold();
		let padded = format!("{label:<width$}");
		let label_styled = padded.if_supports_color(Stream::Stdout, |t| t.style(label_style));
		println!("{indent}{label_styled}{value}");
	}
}
