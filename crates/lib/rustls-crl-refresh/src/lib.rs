//! Process-wide CRL cache plus refreshable rustls verifiers.
//!
//! See the [crate-level README](https://docs.rs/rustls-crl-refresh)
//! for the design rationale. The short version: rustls's
//! `WebPkiClientVerifier` / `WebPkiServerVerifier` bake the CRL list
//! into the verifier at construction time, so refreshing CRL bytes
//! requires rebuilding the surrounding `ServerConfig` /
//! `ClientConfig`. Long-running servers that keep `Arc`-identity-keyed
//! connection pools (hyper-util's `legacy::Client`, `quinn::Endpoint`, …)
//! pay a real cost when those configs churn. This crate keeps the
//! configs stable: a [`CrlCache`] holds the latest bytes per source,
//! and [`RefreshableClientCertVerifier`] /
//! [`RefreshableServerCertVerifier`] reconstruct the inner
//! `WebPkiVerifier` per handshake against the fresh snapshot.

mod cache;
mod verifier;

pub use cache::{
	CrlCache, CrlError, CrlFetchFailure, CrlFetcher, CrlSourceId, dedupe_crl_sources, read_crl_file,
};
pub use verifier::{RefreshableClientCertVerifier, RefreshableServerCertVerifier};
