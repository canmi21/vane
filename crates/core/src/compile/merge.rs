use std::path::PathBuf;

use crate::error::Error;
use crate::rule::RawRule;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RawRuleFile {
	pub path: PathBuf,
	#[serde(default)]
	pub order: i32,
	#[serde(default)]
	pub rules: Vec<RawRule>,
}

#[derive(Debug, Clone)]
pub struct MergedConfig {
	pub rules: Vec<RawRule>,
	pub source_files: Vec<PathBuf>,
}

/// Merge multiple rule files into a single canonical rule set.
///
/// # Errors
/// Returns [`Error::compile`] when two rules across the input files share
/// a `name`.
pub fn merge(mut files: Vec<RawRuleFile>) -> Result<MergedConfig, Error> {
	files.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.path.cmp(&b.path)));

	let mut rules: Vec<RawRule> = Vec::new();
	let mut source_files: Vec<PathBuf> = Vec::with_capacity(files.len());
	for file in files {
		source_files.push(file.path);
		for rule in file.rules {
			if rules.iter().any(|existing| existing.name == rule.name) {
				return Err(Error::compile(format!("duplicate rule name: {:?}", rule.name)));
			}
			rules.push(rule);
		}
	}
	Ok(MergedConfig { rules, source_files })
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::error::ErrorKind;

	fn rule(name: &str) -> RawRule {
		let raw = serde_json::json!({
			"name": name,
			"listen": [":443"],
			"terminate": { "type": "http_proxy" },
		});
		serde_json::from_value(raw).expect("parse rule")
	}

	fn file(path: &str, order: i32, rules: Vec<RawRule>) -> RawRuleFile {
		RawRuleFile { path: PathBuf::from(path), order, rules }
	}

	#[test]
	fn sorts_by_order_then_path_stable() {
		// 09-config.md § _Merge_: stable-sort by (order asc, filename lex).
		let files = vec![
			file("b.json", 10, vec![rule("b")]),
			file("a.json", 10, vec![rule("a")]),
			file("0.json", 0, vec![rule("zero")]),
		];
		let merged = merge(files).expect("merge ok");
		let names: Vec<_> = merged.rules.iter().map(|r| r.name.as_str()).collect();
		assert_eq!(names, vec!["zero", "a", "b"]);
	}

	#[test]
	fn rejects_duplicate_rule_names_with_compile_error() {
		let files = vec![file("a.json", 0, vec![rule("same")]), file("b.json", 1, vec![rule("same")])];
		let err = merge(files).expect_err("duplicate must error");
		assert!(matches!(err.kind(), ErrorKind::Compile));
	}

	#[test]
	fn preserves_every_source_file_path() {
		let files = vec![file("x.json", 0, vec![]), file("y.json", 0, vec![])];
		let merged = merge(files).expect("merge ok");
		assert_eq!(merged.source_files, vec![PathBuf::from("x.json"), PathBuf::from("y.json")],);
	}
}
