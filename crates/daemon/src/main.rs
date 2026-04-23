//! vaned — the daemon. See `spec/architecture/16-crate-layout.md`.

use vane_core::version::{BuildInfo, format_version};

const FEATURES: &[&str] = &[
	#[cfg(feature = "aws-lc-rs")]
	"aws-lc-rs",
	#[cfg(feature = "ring")]
	"ring",
	#[cfg(feature = "h3")]
	"h3",
	#[cfg(feature = "cgi")]
	"cgi",
	#[cfg(feature = "wasm")]
	"wasm",
];

const PROTOCOLS: &[&str] = &[
	"tcp",
	"udp",
	#[cfg(feature = "h3")]
	"quic",
	"h1",
	"h2",
	#[cfg(feature = "h3")]
	"h3",
	"ws",
	#[cfg(feature = "cgi")]
	"cgi",
];

const BUILD_INFO: BuildInfo = BuildInfo {
	version: env!("CARGO_PKG_VERSION"),
	commit: env!("VANE_COMMIT"),
	build_date: env!("VANE_BUILD_DATE"),
	rustc: env!("VANE_RUSTC"),
	cargo: env!("VANE_CARGO"),
	features: FEATURES,
	protocols: PROTOCOLS,
};

fn main() {
	// Placeholder: clap entry to come. For now, --version prints the banner.
	let args: Vec<String> = std::env::args().collect();
	if args.iter().any(|a| a == "--version" || a == "-v") {
		print!("{}", format_version(&BUILD_INFO));
		return;
	}
	eprintln!(
		"vaned placeholder — run with --version (full CLI per spec/architecture/16-crate-layout.md coming)"
	);
}
