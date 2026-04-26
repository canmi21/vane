use std::path::PathBuf;

use crate::error::Error;
use crate::preset::RuleEntry;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RawRuleFile {
	/// Set by `crate::config::scan_rules_dir` from the on-disk filename.
	/// User-authored rule JSON does not include this — the field defaults
	/// to an empty `PathBuf` at parse time and the loader overwrites it.
	#[serde(default)]
	pub path: PathBuf,
	#[serde(default)]
	pub order: i32,
	#[serde(default)]
	pub rules: Vec<RuleEntry>,
}

#[derive(Debug, Clone)]
pub struct MergedConfig {
	/// Unexpanded entries — `RuleEntry::Preset(_)` invocations are still
	/// in their authored form; `expand` runs the dispatcher and produces
	/// the canonical `RawRule` slab.
	pub rules: Vec<RuleEntry>,
	pub source_files: Vec<PathBuf>,
}

/// Merge multiple rule files into a single canonical entry list.
///
/// Files are sorted by `(order asc, path lex)` then concatenated. The
/// duplicate-name check moved to [`crate::compile::expand::expand`] —
/// presets emit synthetic rule names like `<base>.main` and `<base>.ws`
/// that aren't visible until after expansion, so checking here would
/// either miss collisions or false-positive on legitimate preset
/// emissions.
///
/// # Errors
/// Currently infallible. The `Result` shape is preserved so future
/// per-file validation (e.g. cross-file ordering rules) can surface
/// here without a signature change.
pub fn merge(mut files: Vec<RawRuleFile>) -> Result<MergedConfig, Error> {
	files.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.path.cmp(&b.path)));

	let mut rules: Vec<RuleEntry> = Vec::new();
	let mut source_files: Vec<PathBuf> = Vec::with_capacity(files.len());
	for file in files {
		source_files.push(file.path);
		rules.extend(file.rules);
	}
	Ok(MergedConfig { rules, source_files })
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::rule::RawRule;

	fn raw_rule(name: &str) -> RawRule {
		let raw = serde_json::json!({
			"name": name,
			"listen": [":443"],
			"terminate": { "type": "http_proxy" },
		});
		serde_json::from_value(raw).expect("parse rule")
	}

	fn entry(name: &str) -> RuleEntry {
		RuleEntry::Raw(raw_rule(name))
	}

	fn file(path: &str, order: i32, rules: Vec<RuleEntry>) -> RawRuleFile {
		RawRuleFile { path: PathBuf::from(path), order, rules }
	}

	fn entry_name(e: &RuleEntry) -> &str {
		match e {
			RuleEntry::Raw(r) => r.name.as_str(),
			RuleEntry::Preset(inv) => inv.name.as_str(),
		}
	}

	#[test]
	fn sorts_by_order_then_path_stable() {
		// 09-config.md § _Merge_: stable-sort by (order asc, filename lex).
		let files = vec![
			file("b.json", 10, vec![entry("b")]),
			file("a.json", 10, vec![entry("a")]),
			file("0.json", 0, vec![entry("zero")]),
		];
		let merged = merge(files).expect("merge ok");
		let names: Vec<_> = merged.rules.iter().map(entry_name).collect();
		assert_eq!(names, vec!["zero", "a", "b"]);
	}

	#[test]
	fn duplicate_names_pass_through_merge_without_check() {
		// Dup detection now happens at expand time — merge intentionally
		// does NOT short-circuit on identical names, since presets can
		// reasonably collide pre-expansion.
		let files =
			vec![file("a.json", 0, vec![entry("same")]), file("b.json", 1, vec![entry("same")])];
		let merged = merge(files).expect("merge ok — no dup check here");
		assert_eq!(merged.rules.len(), 2);
	}

	#[test]
	fn preserves_every_source_file_path() {
		let files = vec![file("x.json", 0, vec![]), file("y.json", 0, vec![])];
		let merged = merge(files).expect("merge ok");
		assert_eq!(merged.source_files, vec![PathBuf::from("x.json"), PathBuf::from("y.json")]);
	}
}
