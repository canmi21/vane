use crate::compile::merge::MergedConfig;
use crate::error::Error;
use crate::rule::RawRule;

#[derive(Debug, Clone)]
pub struct RawRuleSet {
	pub rules: Vec<RawRule>,
	pub source_files: Vec<std::path::PathBuf>,
}

/// Preset expansion. Today this is a pure identity — real preset expansion
/// (`reverse_proxy`, `port_forward`, `static_site`, `redirect_https`) lands
/// at S1-22 per the roadmap. The signature is in place so later stages can
/// start consuming `RawRuleSet` without a rewrite.
///
/// # Errors
/// Currently infallible; the `Result` shape is kept so preset-args
/// validation in S1-22 can surface compile errors from this stage.
pub fn expand(merged: MergedConfig) -> Result<RawRuleSet, Error> {
	Ok(RawRuleSet { rules: merged.rules, source_files: merged.source_files })
}
