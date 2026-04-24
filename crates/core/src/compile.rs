pub mod analyze;
pub mod expand;
pub mod lower;
pub mod merge;
pub mod validate;

use std::sync::Arc;

use crate::error::Error;
use crate::ir::SymbolicFlowGraph;
use crate::metadata::{FetchMetadataProvider, MiddlewareMetadataProvider};

pub use analyze::{AnalyzedRule, AnalyzedRuleSet, InspectionLevel, Posture};
pub use expand::RawRuleSet;
pub use merge::{MergedConfig, RawRuleFile};

/// Facade for the core compile pipeline.
///
/// Runs `merge → expand → analyze → lower → validate` and returns an
/// `Arc<SymbolicFlowGraph>` ready for `vane-engine::FlowGraph::link`.
///
/// # Errors
/// Returns [`Error::compile`] on duplicate rule names, unknown middleware
/// or fetch names referenced by rules, bad `ListenSpec` strings, predicate
/// type mismatches, or graph-level validation failures (dangling IDs,
/// cycles, phase mismatches).
pub fn compile(
	files: Vec<RawRuleFile>,
	mw_meta: &dyn MiddlewareMetadataProvider,
	fetch_meta: &dyn FetchMetadataProvider,
) -> Result<Arc<SymbolicFlowGraph>, Error> {
	let merged = merge::merge(files)?;
	let expanded = expand::expand(merged)?;
	let analyzed = analyze::analyze(expanded, mw_meta, fetch_meta)?;
	let graph = lower::lower(analyzed, mw_meta, fetch_meta)?;
	validate::validate(&graph)?;
	Ok(Arc::new(graph))
}
