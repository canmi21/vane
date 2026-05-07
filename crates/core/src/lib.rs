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
	/// - URL values (`Homepage:`, `Source:`, `License:`) → green
	/// - `ABSOLUTELY NO WARRANTY` substring → red + bold
	/// - everything else (description, version values, copyright,
	///   licence prose) → plain
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

		let brand = Style::new().yellow().bold();
		let warning = Style::new().red().bold();

		println!();
		println!(
			"{INDENT}{} — {DESCRIPTION}",
			"Vane".if_supports_color(Stream::Stdout, |t| t.style(brand)),
		);
		println!();

		print_label(
			"Built:",
			&format!("{} ({} {})", info.version, info.commit, info.build_date),
			false,
			WIDTH,
			INDENT,
		);
		print_label("Rust:", info.rustc, false, WIDTH, INDENT);
		print_label("Cargo:", info.cargo, false, WIDTH, INDENT);
		if !info.features.is_empty() {
			print_label("Features:", &info.features.join(", "), false, WIDTH, INDENT);
		}
		if !info.protocols.is_empty() {
			print_label("Protocols:", &info.protocols.join(", "), false, WIDTH, INDENT);
		}

		println!();
		println!("{INDENT}{COPYRIGHT}");
		println!();
		println!("{INDENT}Released under the MIT License without restriction.");
		println!(
			"{INDENT}This software comes with {}.",
			"ABSOLUTELY NO WARRANTY".if_supports_color(Stream::Stdout, |t| t.style(warning)),
		);
		println!();

		print_label("Homepage:", HOMEPAGE, true, WIDTH, INDENT);
		print_label("Source:", REPOSITORY, true, WIDTH, INDENT);
		print_label("License:", LICENSE_URL, true, WIDTH, INDENT);
		println!();
	}

	fn print_label(label: &str, value: &str, value_is_url: bool, width: usize, indent: &str) {
		let label_style = Style::new().cyan().bold();
		let url_style = Style::new().green();
		let padded = format!("{label:<width$}");
		let label_styled = padded.if_supports_color(Stream::Stdout, |t| t.style(label_style));
		if value_is_url {
			println!(
				"{indent}{label_styled}{}",
				value.if_supports_color(Stream::Stdout, |t| t.style(url_style)),
			);
		} else {
			println!("{indent}{label_styled}{value}");
		}
	}
}
