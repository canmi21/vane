//! Runtime IR: `FlowGraph` (linked form holding `Vec<MiddlewareInst>` and
//! `Vec<FetchInst>`), `MiddlewareInst`, `FetchInst`, plus `FlowGraph::link`
//! — the feature-gate rejection point.
//!
//! See `spec/architecture/02-flow.md` § _Compile and link — two stages,
//! two crates_. Feature: S1-12.
