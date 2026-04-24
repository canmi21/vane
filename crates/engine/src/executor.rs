//! Iterative walker: owned-slots state machine, `Decision` routing,
//! default fallback tombstones, `LazyBuffer` trigger at the flagged node.
//!
//! Single `async fn`, single state machine, single allocation per
//! request. See `spec/architecture/02-flow.md` § _Execution model_.
//! Feature: S1-15.
