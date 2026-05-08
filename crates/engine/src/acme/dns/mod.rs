//! Re-exports of the [`acme_provider`] crate's `DnsProvider` trait
//! and built-in providers. The trait + error type live in the
//! standalone `acme-provider` crate; vane-engine pulls in the
//! `cloudflare` feature when its own `cloudflare` cargo feature is
//! on, so internal callers can keep using the
//! `vane_engine::acme::dns::*` path without depending on the lib
//! directly.
//!
//! `MockDnsProvider` (an in-process hickory-server-backed impl for
//! integration tests) lives in `vane-testutil` so non-test builds
//! never link the hickory stack.

pub use acme_provider::{DnsProvider, DnsProviderError};

#[cfg(feature = "cloudflare")]
pub use acme_provider::cloudflare::{self, CloudflareConfig, CloudflareDnsProvider};
