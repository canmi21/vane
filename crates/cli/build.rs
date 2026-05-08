//! Emits compile-time env vars consumed by `main.rs` via `env!()`.
//!
//! Contract: see [`spec/crates/cli.md`](../../spec/crates/cli.md).

use std::process::Command;

fn main() {
	println!("cargo:rustc-env=VANE_COMMIT={}", git_short_commit());
	println!("cargo:rustc-env=VANE_BUILD_DATE={}", build_date());
	println!("cargo:rustc-env=VANE_RUSTC={}", rustc_version());
	println!("cargo:rustc-env=VANE_CARGO={}", cargo_version());
	println!("cargo:rerun-if-changed=build.rs");
	println!("cargo:rerun-if-changed=../../.git/HEAD");
}

fn git_short_commit() -> String {
	Command::new("git")
		.args(["rev-parse", "--short=9", "HEAD"])
		.output()
		.ok()
		.filter(|o| o.status.success())
		.map_or_else(|| "unknown".to_owned(), |o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

fn build_date() -> String {
	Command::new("date")
		.args(["-u", "+%Y-%m-%d"])
		.output()
		.ok()
		.filter(|o| o.status.success())
		.map_or_else(|| "unknown".to_owned(), |o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

fn rustc_version() -> String {
	tool_version_tail("rustc")
}

fn cargo_version() -> String {
	tool_version_tail("cargo")
}

/// Capture everything after the tool name on the first line of `<tool> --version`.
///
/// `rustc 1.95.0 (59807616e 2026-04-14)` → `1.95.0 (59807616e 2026-04-14)`
fn tool_version_tail(tool: &str) -> String {
	Command::new(tool)
		.arg("--version")
		.output()
		.ok()
		.filter(|o| o.status.success())
		.and_then(|o| String::from_utf8(o.stdout).ok())
		.map_or_else(
			|| "unknown".to_owned(),
			|s| {
				let prefix = format!("{tool} ");
				s.trim().strip_prefix(&prefix).unwrap_or_else(|| s.trim()).to_owned()
			},
		)
}
