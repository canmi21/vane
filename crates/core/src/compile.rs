//! Core compile pipeline: `merge` → `expand` (preset stubs) → `analyze`
//! → `lower` → IR `validate`. Pure functions: `RawRuleSet` + metadata
//! providers → `Arc<SymbolicFlowGraph>`.
//!
//! See `spec/architecture/02-flow.md` § _Compile and link_,
//! `spec/architecture/09-config.md`, `spec/architecture/14-presets.md`.
//! Feature: S1-09.

pub mod analyze;
pub mod expand;
pub mod lower;
pub mod merge;
pub mod validate;
