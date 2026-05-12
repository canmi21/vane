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
use crate::preset::RuleEntry;
use crate::rule::SourceInfo;

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
		annotate_rule_source_lines(&content, &path, &mut file.rules);
		file.path = path;
		files.push(file);
	}

	Ok(files)
}

/// Walk the raw JSON text to find the start line of every entry in the
/// top-level `rules` array, then stamp `(file, line)` onto each entry's
/// `SourceInfo`. Without this, every rule starts life with an empty
/// `SourceInfo`, and `source_prefix` in `core::compile::lower` collapses
/// to a blank string — diagnostics show errors with no file or line.
fn annotate_rule_source_lines(content: &str, path: &Path, entries: &mut [RuleEntry]) {
	let starts = locate_rule_array_element_lines(content);
	for (idx, entry) in entries.iter_mut().enumerate() {
		let line = starts.get(idx).copied().unwrap_or(0);
		let info = SourceInfo { file: path.to_path_buf(), line };
		match entry {
			RuleEntry::Raw(rule) => rule.source = info,
			RuleEntry::Preset(inv) => inv.source = info,
		}
	}
}

/// Locate the starting 1-based line number of each top-level element in
/// the file-level `"rules": [...]` array. Returns one entry per array
/// element in source order. Heuristic-but-deterministic byte walk with
/// depth/string/escape tracking — sufficient for the structured rule
/// JSON the loader handles.
fn locate_rule_array_element_lines(content: &str) -> Vec<u32> {
	let bytes = content.as_bytes();
	let mut out = Vec::new();
	let Some(rules_key_pos) = find_top_level_key(content, "rules") else {
		return out;
	};

	// Skip `"rules"` + `:` + whitespace to the opening `[`.
	let mut i = rules_key_pos;
	while i < bytes.len() && bytes[i] != b'[' {
		i += 1;
	}
	if i >= bytes.len() {
		return out;
	}
	i += 1;

	let mut depth: i32 = 0;
	let mut in_string = false;
	let mut escape = false;
	let mut element_started = false;

	while i < bytes.len() {
		let c = bytes[i];
		if in_string {
			if escape {
				escape = false;
			} else if c == b'\\' {
				escape = true;
			} else if c == b'"' {
				in_string = false;
			}
			i += 1;
			continue;
		}
		match c {
			b'"' => {
				if depth == 0 && !element_started {
					out.push(line_at(content, i));
					element_started = true;
				}
				in_string = true;
			}
			b'{' | b'[' => {
				if depth == 0 && !element_started {
					out.push(line_at(content, i));
					element_started = true;
				}
				depth += 1;
			}
			b'}' | b']' => {
				depth -= 1;
				if depth < 0 {
					return out;
				}
			}
			b',' if depth == 0 => element_started = false,
			b' ' | b'\t' | b'\r' | b'\n' => {}
			_ => {
				if depth == 0 && !element_started {
					out.push(line_at(content, i));
					element_started = true;
				}
			}
		}
		i += 1;
	}
	out
}

/// Return the byte offset just past the value side of a top-level key
/// (depth-1) inside the outermost JSON object. Yields `None` if the key
/// is not present at the file's top level.
fn find_top_level_key(content: &str, key: &str) -> Option<usize> {
	let bytes = content.as_bytes();
	let mut i = 0;
	// Skip leading whitespace to the outermost '{'.
	while i < bytes.len() && bytes[i].is_ascii_whitespace() {
		i += 1;
	}
	if i >= bytes.len() || bytes[i] != b'{' {
		return None;
	}
	i += 1;
	let target = format!("\"{key}\"");
	let tbytes = target.as_bytes();
	let mut depth: i32 = 0;
	let mut in_string = false;
	let mut escape = false;
	while i < bytes.len() {
		let c = bytes[i];
		if in_string {
			if escape {
				escape = false;
			} else if c == b'\\' {
				escape = true;
			} else if c == b'"' {
				in_string = false;
			}
			i += 1;
			continue;
		}
		if c == b'"' {
			// Check if at depth 0 of the outer object (depth==0 here is
			// "inside outer object, not yet inside any nested struct").
			if depth == 0 && i + tbytes.len() <= bytes.len() && &bytes[i..i + tbytes.len()] == tbytes {
				return Some(i + tbytes.len());
			}
			in_string = true;
		} else if c == b'{' || c == b'[' {
			depth += 1;
		} else if c == b'}' || c == b']' {
			depth -= 1;
		}
		i += 1;
	}
	None
}

fn line_at(content: &str, byte_offset: usize) -> u32 {
	let mut line: u32 = 1;
	for b in content.as_bytes().iter().take(byte_offset) {
		if *b == b'\n' {
			line = line.saturating_add(1);
		}
	}
	line
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
	fn scan_rules_dir_threads_rule_source_lines_into_each_entry() {
		let tmp = tempfile::tempdir().expect("tempdir");
		// Each rule object starts at a distinct line; the loader must
		// stamp that line onto the entry's SourceInfo so downstream
		// `source_prefix` carries `file:line` into diagnostics.
		let body = "{\n  \"rules\": [\n    { \"name\": \"a\", \"listen\": [\":1\"], \"terminate\": { \"type\": \"http_proxy\" } },\n    { \"name\": \"b\", \"listen\": [\":2\"], \"terminate\": { \"type\": \"http_proxy\" } }\n  ]\n}\n";
		write_json(tmp.path(), "rules.json", body);

		let files = scan_rules_dir(tmp.path()).expect("scan ok");
		assert_eq!(files.len(), 1);
		assert_eq!(files[0].rules.len(), 2);
		for (entry, expected_line) in files[0].rules.iter().zip([3u32, 4u32]) {
			match entry {
				RuleEntry::Raw(rule) => {
					assert_eq!(rule.source.line, expected_line);
					assert_eq!(rule.source.file.file_name().and_then(|s| s.to_str()), Some("rules.json"));
				}
				RuleEntry::Preset(_) => panic!("expected Raw entry"),
			}
		}
	}

	#[test]
	fn locate_rule_array_element_lines_handles_nested_args_objects() {
		let body = r#"{
  "order": 0,
  "rules": [
    { "name": "first", "listen": [":1"], "terminate": { "type": "http_proxy", "args": { "nested": ["x", "y"] } } },
    {
      "name": "second",
      "listen": [":2"],
      "terminate": { "type": "http_proxy" }
    }
  ]
}
"#;
		let lines = locate_rule_array_element_lines(body);
		assert_eq!(lines, vec![4, 5]);
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
