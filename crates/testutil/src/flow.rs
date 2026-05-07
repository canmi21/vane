//! `build_flow(rules)` helper — constructs a linked `FlowGraph` from
//! raw rule bytes without touching disk or spawning a daemon. Used by
//! unit tests that need an executor target.
//!
//! See [`spec/crates/core.md` § _Compile pipeline_](../../../spec/crates/core.md#compile-pipeline).
//
// TODO(testutil-build-flow): module is documentation-only; integration
// tests currently inline equivalents. Land the shared helper here when
// more than one test needs the same shape.
