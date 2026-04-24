//! `build_flow(rules)` helper — constructs a linked `FlowGraph` from
//! raw rule bytes without touching disk or spawning a daemon. Used by
//! unit tests that need an executor target.
//!
//! See `spec/architecture/16-crate-layout.md` § _`vane-testutil`_.
