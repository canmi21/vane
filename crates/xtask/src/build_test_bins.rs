// Build the workspace's test-driver binaries (`vane`, `vaned`) and write
// their paths to nextest's per-run env file (VANE_BIN, VANED_BIN) so E2E
// test processes spawn them without paying a runtime `cargo build` (and
// the cargo-lock contention that would imply across parallel test
// processes).
//
// Driven by `.config/nextest.toml`'s `build-test-bins` setup script.
// Stand-alone runs work too: point `NEXTEST_ENV` at a scratch file
// (`NEXTEST_ENV=/tmp/env cargo xtask build-test-bins`) and inspect the
// resulting `VANE_BIN=` / `VANED_BIN=` lines.
//
// Path extraction goes through `cargo build --message-format=json`
// rather than hard-coding `target/debug/<bin>` so the script keeps
// working under `CARGO_TARGET_DIR` overrides, `--target <triple>`
// cross-compilation, and `--release` invocations. cargo emits one JSON
// object per line; we keep the one whose `target.name` matches the
// package, `target.kind` contains `bin`, and `executable` is set.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

#[derive(Deserialize)]
struct Artifact {
	reason: String,
	target: Target,
	executable: Option<String>,
}

#[derive(Deserialize)]
struct Target {
	name: String,
	kind: Vec<String>,
}

/// (package == bin name, env var) pairs to build and export.
const BINS: &[(&str, &str)] = &[("vane", "VANE_BIN"), ("vaned", "VANED_BIN")];

pub(crate) fn run() -> Result<()> {
	let nextest_env = std::env::var_os("NEXTEST_ENV")
		.ok_or_else(|| anyhow!("NEXTEST_ENV is unset; this command runs as a nextest setup script"))?;
	let env_path = PathBuf::from(&nextest_env);

	for (pkg, env_var) in BINS {
		let bin = build_bin(pkg)?;
		let mut env_file = OpenOptions::new()
			.append(true)
			.create(true)
			.open(&env_path)
			.with_context(|| format!("open NEXTEST_ENV file at {}", env_path.display()))?;
		writeln!(env_file, "{env_var}={bin}").with_context(|| format!("write {env_var}"))?;
	}
	Ok(())
}

/// Build `pkg`'s same-named binary and return its absolute path.
fn build_bin(pkg: &str) -> Result<String> {
	let output = Command::new("cargo")
		.args(["build", "-p", pkg, "--bin", pkg, "--message-format=json", "--quiet"])
		.stderr(Stdio::inherit())
		.output()
		.with_context(|| format!("invoking `cargo build` for {pkg}"))?;
	if !output.status.success() {
		bail!("cargo build for {pkg} exited non-zero");
	}

	let bin = output
		.stdout
		.split(|&b| b == b'\n')
		.filter_map(|line| serde_json::from_slice::<Artifact>(line).ok())
		.find_map(|a| {
			(a.reason == "compiler-artifact"
				&& a.target.name == pkg
				&& a.target.kind.iter().any(|k| k == "bin"))
			.then_some(a.executable)
			.flatten()
		})
		.with_context(|| format!("could not extract {pkg} binary path from cargo build output"))?;

	if !Path::new(&bin).is_file() {
		bail!("extracted path {bin} for {pkg} is not a file");
	}
	Ok(bin)
}
