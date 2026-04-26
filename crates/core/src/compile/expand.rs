use std::collections::HashSet;

use crate::compile::merge::MergedConfig;
use crate::error::Error;
use crate::preset::{RuleEntry, expand_invocation};
use crate::rule::RawRule;

#[derive(Debug, Clone)]
pub struct RawRuleSet {
	pub rules: Vec<RawRule>,
	pub source_files: Vec<std::path::PathBuf>,
}

/// Preset expansion. Walks the merged `RuleEntry` list, dispatching
/// each `Preset(inv)` to its expander and passing `Raw(r)` through
/// verbatim. After concatenation, enforces uniqueness across the full
/// post-expansion `RawRule` name set — presets can synthesise names
/// (`<base>.main`, `<base>.ws-allow`, etc.) that only become visible
/// here, so this is the right layer for the dup check.
///
/// # Errors
/// Returns [`Error::compile`] for unknown preset names, preset arg
/// validation failures, or duplicate rule names after expansion.
pub fn expand(merged: MergedConfig) -> Result<RawRuleSet, Error> {
	let mut rules: Vec<RawRule> = Vec::new();
	for entry in merged.rules {
		match entry {
			RuleEntry::Raw(r) => rules.push(r),
			RuleEntry::Preset(inv) => rules.extend(expand_invocation(inv)?),
		}
	}

	let mut seen: HashSet<&str> = HashSet::with_capacity(rules.len());
	for r in &rules {
		if !seen.insert(r.name.as_str()) {
			return Err(Error::compile(format!(
				"duplicate rule name after preset expansion: {:?}",
				r.name
			)));
		}
	}

	Ok(RawRuleSet { rules, source_files: merged.source_files })
}

#[cfg(test)]
mod tests {
	use std::path::PathBuf;

	use super::*;
	use crate::preset::{PresetInvocation, RuleEntry};
	use crate::rule::{RawRule, SourceInfo};

	fn raw(name: &str) -> RawRule {
		let raw = serde_json::json!({
			"name": name,
			"listen": [":443"],
			"terminate": { "type": "http_proxy" },
		});
		serde_json::from_value(raw).expect("parse rule")
	}

	fn port_forward_invocation(name: &str) -> PresetInvocation {
		PresetInvocation {
			name: name.to_string(),
			preset: "port_forward".to_string(),
			listen: vec![":2222".into()],
			args: serde_json::json!({ "upstream": "10.0.0.5:22" }),
			tls: None,
			source: SourceInfo::default(),
		}
	}

	fn merged(rules: Vec<RuleEntry>) -> MergedConfig {
		MergedConfig { rules, source_files: vec![PathBuf::from("rules/x.json")] }
	}

	#[test]
	fn expand_passes_through_raw_only_input() {
		let m = merged(vec![RuleEntry::Raw(raw("a")), RuleEntry::Raw(raw("b"))]);
		let out = expand(m).expect("expand");
		let names: Vec<_> = out.rules.iter().map(|r| r.name.as_str()).collect();
		assert_eq!(names, vec!["a", "b"]);
	}

	#[test]
	fn expand_concatenates_raw_and_preset_entries() {
		let m = merged(vec![
			RuleEntry::Raw(raw("first")),
			RuleEntry::Preset(port_forward_invocation("fwd")),
			RuleEntry::Raw(raw("last")),
		]);
		let out = expand(m).expect("expand");
		let names: Vec<_> = out.rules.iter().map(|r| r.name.as_str()).collect();
		assert_eq!(names, vec!["first", "fwd", "last"]);
	}

	#[test]
	fn expand_detects_dup_name_after_preset_expansion() {
		// Two reverse_proxy presets with the same `name` both emit `<name>.main` —
		// expansion-side dup check catches it.
		let inv_a = PresetInvocation {
			name: "api".to_string(),
			preset: "reverse_proxy".to_string(),
			listen: vec![":443".into()],
			args: serde_json::json!({ "upstream": "u:1" }),
			tls: None,
			source: SourceInfo::default(),
		};
		let inv_b = PresetInvocation {
			name: "api".to_string(),
			preset: "reverse_proxy".to_string(),
			listen: vec![":443".into()],
			args: serde_json::json!({ "upstream": "u:2" }),
			tls: None,
			source: SourceInfo::default(),
		};
		let m = merged(vec![RuleEntry::Preset(inv_a), RuleEntry::Preset(inv_b)]);
		let err = expand(m).expect_err("dup must surface");
		let msg = err.to_string();
		assert!(msg.contains("duplicate"), "error mentions duplicate: {msg}");
		assert!(msg.contains("api"), "error names the offending base name: {msg}");
	}

	#[test]
	fn expand_preserves_source_files() {
		let m = merged(vec![RuleEntry::Raw(raw("a"))]);
		let out = expand(m).expect("expand");
		assert_eq!(out.source_files, vec![PathBuf::from("rules/x.json")]);
	}
}
