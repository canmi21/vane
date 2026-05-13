//! Emits compile-time env vars consumed by `main.rs` via `env!()`.
//!
//! Contract: see `spec/crates/daemon.md` ("build.rs contract").
//!
//! Reproducibility: `VANE_BUILD_DATE` resolves in priority order from
//! `SOURCE_DATE_EPOCH` (the cross-distro reproducible-builds convention),
//! then the unix timestamp of the current `HEAD` commit, and finally the
//! current wall-clock time. No subprocess spawn for `date(1)`.

// The Howard-Hinnant epoch-to-civil conversion in `civil_from_days`
// does a handful of `i64 <-> u64 <-> u32` casts that are sound for
// the date range vane will ever encounter (1970-01-01 .. 2999-12-31)
// but trip the workspace's default-deny cast lints. The shape of the
// algorithm is well-known and verified against POSIX `date`; allow
// the casts at the file level instead of papering each one over.
#![allow(
	clippy::cast_possible_truncation,
	clippy::cast_possible_wrap,
	clippy::cast_sign_loss,
	clippy::map_unwrap_or
)]

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
	println!("cargo:rustc-env=VANE_COMMIT={}", git_short_commit());
	println!("cargo:rustc-env=VANE_BUILD_DATE={}", format_yyyymmdd(build_date_unix()));
	println!("cargo:rustc-env=VANE_RUSTC={}", rustc_version());
	println!("cargo:rustc-env=VANE_CARGO={}", cargo_version());
	println!("cargo:rerun-if-changed=build.rs");
	println!("cargo:rerun-if-changed=../../.git/HEAD");
	println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}

fn git_short_commit() -> String {
	Command::new("git")
		.args(["rev-parse", "--short=9", "HEAD"])
		.output()
		.ok()
		.filter(|o| o.status.success())
		.map_or_else(|| "unknown".to_owned(), |o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// Unix seconds for the build-date stamp. Order of preference:
/// 1. `SOURCE_DATE_EPOCH` — explicit reproducible-builds override.
/// 2. `git log -1 --format=%ct HEAD` — commit timestamp.
/// 3. `SystemTime::now()` — wall-clock fallback for non-git builds
///    (e.g. tarball / vendored sources).
fn build_date_unix() -> i64 {
	if let Ok(s) = std::env::var("SOURCE_DATE_EPOCH")
		&& let Ok(n) = s.trim().parse::<i64>()
	{
		return n;
	}
	if let Some(n) = git_commit_unix() {
		return n;
	}
	SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn git_commit_unix() -> Option<i64> {
	Command::new("git")
		.args(["log", "-1", "--format=%ct", "HEAD"])
		.output()
		.ok()
		.filter(|o| o.status.success())
		.and_then(|o| String::from_utf8(o.stdout).ok())
		.and_then(|s| s.trim().parse::<i64>().ok())
}

/// Render a unix timestamp as `YYYY-MM-DD` in UTC. Hand-rolled to keep
/// the build script dependency-free (a `time = "0.3"` build-dep would
/// dominate the daemon's clean-build time).
fn format_yyyymmdd(unix: i64) -> String {
	let days = unix.div_euclid(86_400);
	let (y, m, d) = civil_from_days(days);
	format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days` — converts a unix-epoch day count
/// to a `(year, month, day)` proleptic Gregorian triple. Verified
/// against POSIX `date -u -d @<epoch> +%F` for the range
/// `[1970-01-01, 2999-12-31]`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
	let z = z + 719_468;
	let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
	let doe = (z - era * 146_097) as u64;
	let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
	let y = yoe as i64 + era * 400;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
	let mp = (5 * doy + 2) / 153;
	let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
	let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
	let y = if m <= 2 { y + 1 } else { y };
	(y, m, d)
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
