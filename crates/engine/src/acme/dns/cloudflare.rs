//! Cloudflare DNS API [`super::DnsProvider`] implementation.
//!
//! The actual provider lands in a follow-up commit; this module
//! exists from the trait-introduction commit so the
//! `#[cfg(feature = "cloudflare")]` gate has a stable target and
//! the `cloudflare` feature can be enabled without dead-code
//! warnings on subsequent commits.

#![allow(dead_code, reason = "implementation lands in the cloudflare provider commit")]
