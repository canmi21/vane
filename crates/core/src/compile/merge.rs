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
