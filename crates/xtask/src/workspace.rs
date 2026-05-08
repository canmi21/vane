// Locate the workspace root by asking cargo. Used by every
// subcommand that touches files relative to the workspace.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

pub(crate) fn root() -> Result<PathBuf> {
	let output = Command::new("cargo")
		.args(["locate-project", "--workspace", "--message-format=plain"])
		.output()
		.context("invoking `cargo locate-project`")?;
	if !output.status.success() {
		bail!("`cargo locate-project --workspace` exited non-zero");
	}
	let path = String::from_utf8(output.stdout).context("non-utf8 cargo output")?;
	let manifest = PathBuf::from(path.trim());
	manifest.parent().map(PathBuf::from).context("workspace manifest has no parent directory")
}
