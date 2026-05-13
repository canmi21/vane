//! Shared `--version` banner for the vane CLI and daemon binaries.
//!
//! `vane-core` exposes [`BuildInfo`] as pure data (no presentation
//! deps). This crate owns the [`owo-colors`]-driven printer that both
//! `vane` and `vaned` invoke from their respective `main`s.
//!
//! [`owo-colors`]: https://crates.io/crates/owo-colors

use owo_colors::{OwoColorize, Stream, Style};
use vane_core::meta::{COPYRIGHT, DESCRIPTION, HOMEPAGE, LICENSE_URL, REPOSITORY};
use vane_core::version::BuildInfo;

/// Print the shared build banner used by both `vane -v` and
/// `vaned -v`. Goes straight to stdout. ANSI colour escapes are
/// emitted only when stdout is detected as a TTY (via owo-colors'
/// `Stream::Stdout` check), so `vane -v | cat` still produces flat
/// ASCII.
///
/// Palette (kept consistent with `vane`'s clap help output):
/// - **Vane** brand → yellow + bold
/// - section labels (`Built:`, `Rust:`, `Homepage:` …) → cyan + bold
/// - MIT-licence prose lead-in → green-tinted accents
/// - `ABSOLUTELY NO WARRANTY` substring → red + bold
/// - everything else → plain
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
