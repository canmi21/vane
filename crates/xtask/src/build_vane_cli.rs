// Build the workspace's `vane` CLI binary and write its path to
// nextest's per-run env file so daemon mgmt tests can spawn the
// CLI without paying a runtime `cargo build` (and the cargo lock
// contention that would imply across parallel test processes).
//
// Driven by `.config/nextest.toml`'s `build-vane-cli` setup script.
// Stand-alone runs work too: point `NEXTEST_ENV` at a scratch file
// (`NEXTEST_ENV=/tmp/env cargo xtask build-vane-cli`) and inspect
// the resulting `VANE_BIN=` line.
//
// Path extraction goes through `cargo build --message-format=json`
// rather than hard-coding `target/debug/vane` so the script keeps
// working under `CARGO_TARGET_DIR` overrides, `--target <triple>`
// cross-compilation, and `--release` invocations. cargo emits one
// JSON object per line; we keep the one whose `target.name` is
// `vane`, `target.kind` contains `bin`, and `executable` is set.

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

pub(crate) fn run() -> Result<()> {
	let nextest_env = std::env::var_os("NEXTEST_ENV")
		.ok_or_else(|| anyhow!("NEXTEST_ENV is unset; this command runs as a nextest setup script"))?;

	let output = Command::new("cargo")
		.args(["build", "-p", "vane", "--bin", "vane", "--message-format=json", "--quiet"])
		.stderr(Stdio::inherit())
		.output()
		.context("invoking `cargo build` for vane")?;
	if !output.status.success() {
		bail!("cargo build exited non-zero");
	}

	let bin = output
		.stdout
		.split(|&b| b == b'\n')
		.filter_map(|line| serde_json::from_slice::<Artifact>(line).ok())
		.find_map(|a| {
			(a.reason == "compiler-artifact"
				&& a.target.name == "vane"
				&& a.target.kind.iter().any(|k| k == "bin"))
			.then_some(a.executable)
			.flatten()
		})
		.context("could not extract vane binary path from cargo build output")?;

	if !Path::new(&bin).is_file() {
		bail!("extracted path {bin} is not a file");
	}

	let env_path = PathBuf::from(&nextest_env);
	let mut env_file = OpenOptions::new()
		.append(true)
		.create(true)
		.open(&env_path)
		.with_context(|| format!("open NEXTEST_ENV file at {}", env_path.display()))?;
	writeln!(env_file, "VANE_BIN={bin}").context("write VANE_BIN to NEXTEST_ENV")?;
	Ok(())
}
