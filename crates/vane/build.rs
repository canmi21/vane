//! Emits compile-time env vars consumed by `main.rs` via `env!()`.
//!
//! Contract: see `spec/architecture/16-crate-layout.md` ("build.rs contract").

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
		.args(["rev-parse", "--short", "HEAD"])
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
	Command::new("rustc")
		.arg("--version")
		.output()
		.ok()
		.filter(|o| o.status.success())
		.and_then(|o| String::from_utf8(o.stdout).ok())
		.and_then(|s| s.split_whitespace().nth(1).map(str::to_owned))
		.unwrap_or_else(|| "unknown".to_owned())
}

fn cargo_version() -> String {
	Command::new("cargo")
		.arg("--version")
		.output()
		.ok()
		.filter(|o| o.status.success())
		.and_then(|o| String::from_utf8(o.stdout).ok())
		.and_then(|s| s.split_whitespace().nth(1).map(str::to_owned))
		.unwrap_or_else(|| "unknown".to_owned())
}
