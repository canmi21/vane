//! Test helpers shared across integration tests. Dev-only, never linked into release.
//!
//! See `spec/testing.md` and `spec/architecture/16-crate-layout.md` §
//! _`vane-testutil`_. Feature: S1-33 (baseline).

pub mod echo;
pub mod flow;
pub mod port;
pub mod tracing;
pub mod vaned_fixture;
