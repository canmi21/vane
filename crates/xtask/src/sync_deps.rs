// Sync the `version = "..."` field of every path-based entry in
// the root Cargo.toml's `[workspace.dependencies]` section against
// the version each crate declares for itself.
//
// Source of truth:
//   - `crates/lib/<name>`  → that crate's own `[package].version`.
//   - `crates/<name>`      → typically `version.workspace = true`,
//                            so it inherits `[workspace.package].version`.
//
// Two modes:
//   Mode::Check   exit non-zero on drift, listing stale entries.
//   Mode::Write   rewrite the root Cargo.toml in place via toml_edit
//                 so the original formatting (comments, ordering,
//                 inline-table layout) is preserved.

use std::path::Path;

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item, Value};

use crate::workspace;

#[derive(Clone, Copy)]
pub(crate) enum Mode {
	Check,
	Write,
}

struct Drift {
	name: String,
	current: String,
	expected: String,
}

pub(crate) fn run(mode: Mode) -> Result<()> {
	let root = workspace::root()?;
	let root_cargo = root.join("Cargo.toml");
	let original = std::fs::read_to_string(&root_cargo)
		.with_context(|| format!("reading {}", root_cargo.display()))?;
	let mut doc: DocumentMut = original.parse().context("parsing root Cargo.toml")?;

	let ws_pkg_version = doc
		.get("workspace")
		.and_then(|w| w.get("package"))
		.and_then(|p| p.get("version"))
		.and_then(Item::as_str)
		.context("[workspace.package].version not found in root Cargo.toml")?
		.to_string();

	let deps = doc
		.get_mut("workspace")
		.and_then(|w| w.get_mut("dependencies"))
		.and_then(Item::as_table_mut)
		.context("[workspace.dependencies] not found in root Cargo.toml")?;

	let mut drift: Vec<Drift> = Vec::new();
	for (key, item) in deps.iter_mut() {
		let name = key.get().to_string();
		let Some(table) = item.as_inline_table_mut() else {
			continue;
		};
		let Some(rel_path) = table.get("path").and_then(Value::as_str) else {
			continue;
		};
		let Some(current) = table.get("version").and_then(Value::as_str) else {
			continue;
		};
		let crate_dir = root.join(rel_path);
		let Some(expected) = read_crate_version(&crate_dir, &ws_pkg_version)? else {
			continue;
		};
		if current == expected {
			continue;
		}
		drift.push(Drift { name, current: current.to_string(), expected: expected.clone() });
		if matches!(mode, Mode::Write)
			&& let Some(slot) = table.get_mut("version")
		{
			*slot = Value::from(expected);
		}
	}

	if drift.is_empty() {
		println!("all workspace dep versions in sync");
		return Ok(());
	}

	match mode {
		Mode::Check => {
			eprintln!("workspace dep version drift:");
			for d in &drift {
				eprintln!("  {:<30} {} -> {}", d.name, d.current, d.expected);
			}
			eprintln!();
			eprintln!("run: just sync-deps   (or cargo xtask sync-deps write)");
			bail!("workspace dep versions out of sync");
		}
		Mode::Write => {
			std::fs::write(&root_cargo, doc.to_string())
				.with_context(|| format!("writing {}", root_cargo.display()))?;
			println!("synced workspace dep versions:");
			for d in &drift {
				println!("  {:<30} {} -> {}", d.name, d.current, d.expected);
			}
			Ok(())
		}
	}
}

fn read_crate_version(crate_dir: &Path, ws_pkg_version: &str) -> Result<Option<String>> {
	let cargo = crate_dir.join("Cargo.toml");
	if !cargo.is_file() {
		return Ok(None);
	}
	let content =
		std::fs::read_to_string(&cargo).with_context(|| format!("reading {}", cargo.display()))?;
	let doc: DocumentMut = content.parse().with_context(|| format!("parsing {}", cargo.display()))?;
	let Some(pkg) = doc.get("package").and_then(Item::as_table_like) else {
		return Ok(None);
	};
	let Some(version) = pkg.get("version") else {
		return Ok(None);
	};

	if let Some(literal) = version.as_str() {
		return Ok(Some(literal.to_string()));
	}
	if let Some(table) = version.as_table_like() {
		let inherits =
			table.get("workspace").and_then(Item::as_value).and_then(Value::as_bool) == Some(true);
		if inherits {
			return Ok(Some(ws_pkg_version.to_string()));
		}
	}
	Ok(None)
}
