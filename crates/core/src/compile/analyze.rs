//! Per-rule analysis: inspection level, specificity, and the two
//! `LazyBuffer` tracks (request / response) that drive `collect_body_before`
//! placement during `lower`.
//!
//! See `spec/architecture/02-flow.md` § _analyze_ and § _`LazyBuffer`_.
