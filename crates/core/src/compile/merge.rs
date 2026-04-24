//! Deterministic multi-file rule merge. Duplicate `rule` names are an
//! error at merge time. Global settings last-write-wins with a merge log.
//!
//! See `spec/architecture/02-flow.md` § _Merge_ and
//! `spec/architecture/09-config.md` § _Merge_.
