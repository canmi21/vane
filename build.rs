/* build.rs */

use chrono::Utc;
use std::process::Command;
use std::str;

fn main() {
	// Get git commit hash.
	let git_hash = get_command_output("git", &["rev-parse", "--short", "HEAD"]);
	println!("cargo:rustc-env=GIT_COMMIT_SHORT={}", git_hash);

	// Get full rustc version string.
	let rustc_version = get_command_output("rustc", &["--version"]);
	println!("cargo:rustc-env=RUSTC_FULL_VERSION={}", rustc_version);

	// Get full cargo version string.
	let cargo_version = get_command_output("cargo", &["--version"]);
	println!("cargo:rustc-env=CARGO_FULL_VERSION={}", cargo_version);

	// Get build date in YYYY-MM-DD format.
	let build_date = Utc::now().format("%Y-%m-%d").to_string();
	println!("cargo:rustc-env=BUILD_DATE={}", build_date);

	// Rerun if git HEAD changes.
	println!("cargo:rerun-if-changed=.git/HEAD");
}

/// Helper to execute a command and return its trimmed stdout.
fn get_command_output(cmd: &str, args: &[&str]) -> String {
	let output = Command::new(cmd)
		.args(args)
		.output()
		.unwrap_or_else(|e| panic!("Failed to execute command '{}': {}", cmd, e));

	str::from_utf8(&output.stdout)
		.unwrap_or("unknown")
		.trim()
		.to_string()
}
