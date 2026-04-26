//! Filesystem scan: `<config_dir>/rules/*.json` → `Vec<RawRuleFile>`.
//!
//! Sub-directories, hidden files, and non-`.json` extensions are
//! silently skipped — operators frequently leave editor swap files
//! (`*.swp`), READMEs, or symlinked sub-directories alongside rule
//! files, and surfacing those as errors would block startup on benign
//! state.

use std::fs;
use std::path::Path;

use crate::compile::merge::RawRuleFile;
use crate::error::Error;

/// Scan a directory for `*.json` rule files. Returns one
/// [`RawRuleFile`] per discovered file with `path` populated from the
/// on-disk filename. Order of the returned vector is unspecified — the
/// merge stage sorts by `(order asc, path lex)` so the loader does not
/// pre-sort.
///
/// # Errors
/// Returns [`Error::compile`] when:
/// - `rules_dir` does not exist (an empty directory is fine, but a
///   missing one is operator error and should fail loud).
/// - `rules_dir` exists but is not a directory.
/// - any `.json` file fails to parse as `RawRuleFile`.
///
/// Returns [`Error::io`] for filesystem-level read failures (permission
/// denied, broken symlink during traversal, etc.).
pub fn scan_rules_dir(rules_dir: &Path) -> Result<Vec<RawRuleFile>, Error> {
	if !rules_dir.exists() {
		return Err(Error::compile(format!("rules directory not found: {}", rules_dir.display())));
	}
	if !rules_dir.is_dir() {
		return Err(Error::compile(format!("rules path is not a directory: {}", rules_dir.display())));
	}

	let mut files = Vec::new();
	let entries = fs::read_dir(rules_dir)
		.map_err(|e| Error::io(format!("read_dir {}: {e}", rules_dir.display())))?;

	for entry in entries {
		let entry = entry.map_err(|e| Error::io(format!("dir entry: {e}")))?;
		let path = entry.path();
		if !path.is_file() {
			continue;
		}
		if path.extension().and_then(|s| s.to_str()) != Some("json") {
			continue;
		}
		let content =
			fs::read_to_string(&path).map_err(|e| Error::io(format!("read {}: {e}", path.display())))?;
		let mut file: RawRuleFile = serde_json::from_str(&content)
			.map_err(|e| Error::compile(format!("parse {}: {e}", path.display())))?;
		file.path = path;
		files.push(file);
	}

	Ok(files)
}

#[cfg(test)]
mod tests {
	use std::fs;

	use super::*;

	fn write_json(dir: &Path, name: &str, body: &str) {
		fs::write(dir.join(name), body).expect("write json");
	}

	fn minimal_rule_file_json() -> &'static str {
		r#"{ "order": 5, "rules": [] }"#
	}

	#[test]
	fn scan_rules_dir_reads_multiple_json_files() {
		let tmp = tempfile::tempdir().expect("tempdir");
		write_json(tmp.path(), "00-a.json", minimal_rule_file_json());
		write_json(tmp.path(), "10-b.json", minimal_rule_file_json());

		let files = scan_rules_dir(tmp.path()).expect("scan ok");
		assert_eq!(files.len(), 2);
		// Path field is populated from on-disk path.
		let names: std::collections::HashSet<_> =
			files.iter().filter_map(|f| f.path.file_name().and_then(|s| s.to_str())).collect();
		assert!(names.contains("00-a.json"));
		assert!(names.contains("10-b.json"));
	}

	#[test]
	fn scan_rules_dir_skips_non_json_extensions() {
		let tmp = tempfile::tempdir().expect("tempdir");
		write_json(tmp.path(), "rule.json", minimal_rule_file_json());
		fs::write(tmp.path().join("README.md"), "docs").unwrap();
		fs::write(tmp.path().join(".rule.json.swp"), "vim swap").unwrap();

		let files = scan_rules_dir(tmp.path()).expect("scan ok");
		assert_eq!(files.len(), 1, "only the .json file is returned");
	}

	#[test]
	fn scan_rules_dir_skips_subdirectories() {
		let tmp = tempfile::tempdir().expect("tempdir");
		fs::create_dir(tmp.path().join("nested")).unwrap();
		write_json(&tmp.path().join("nested"), "ignored.json", minimal_rule_file_json());
		write_json(tmp.path(), "kept.json", minimal_rule_file_json());

		let files = scan_rules_dir(tmp.path()).expect("scan ok");
		assert_eq!(files.len(), 1);
	}

	#[test]
	fn scan_rules_dir_empty_directory_returns_empty_vec() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let files = scan_rules_dir(tmp.path()).expect("scan ok");
		assert!(files.is_empty());
	}

	#[test]
	fn scan_rules_dir_missing_directory_errors() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let missing = tmp.path().join("does-not-exist");
		let err = scan_rules_dir(&missing).expect_err("missing dir errors");
		let msg = err.to_string();
		assert!(msg.contains("not found"), "{msg}");
		assert!(msg.contains("does-not-exist"), "error names the path: {msg}");
	}

	#[test]
	fn scan_rules_dir_path_pointing_at_file_errors() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let file = tmp.path().join("not-a-dir");
		fs::write(&file, "hi").unwrap();
		let err = scan_rules_dir(&file).expect_err("file path rejected");
		assert!(err.to_string().contains("not a directory"), "{err}");
	}

	#[test]
	fn scan_rules_dir_invalid_json_errors_with_path() {
		let tmp = tempfile::tempdir().expect("tempdir");
		write_json(tmp.path(), "broken.json", "{ this is not json");
		let err = scan_rules_dir(tmp.path()).expect_err("bad json rejected");
		let msg = err.to_string();
		assert!(msg.contains("parse"), "error mentions parse: {msg}");
		assert!(msg.contains("broken.json"), "error names the offending file: {msg}");
	}

	#[test]
	fn scan_rules_dir_populates_path_field_with_full_path() {
		let tmp = tempfile::tempdir().expect("tempdir");
		write_json(tmp.path(), "abs.json", minimal_rule_file_json());
		let files = scan_rules_dir(tmp.path()).expect("scan ok");
		assert_eq!(files.len(), 1);
		assert!(files[0].path.is_absolute() || files[0].path.starts_with(tmp.path()));
		assert_eq!(files[0].path.file_name().and_then(|s| s.to_str()), Some("abs.json"));
	}
}
