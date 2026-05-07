//! `DnsProvider` trait + impls for the ACME DNS-01 challenge.
//!
//! Per `spec/acme.md` § _Challenge: DNS-01_ + § _`DnsProvider`
//! trait_. The trait is intentionally narrow: a provider performs
//! TXT-record CRUD against an authoritative DNS API and confirms
//! propagation. The registry's `issue_dns01` orchestrator owns the
//! cleanup ordering, retry budget, and ACME finalization — providers
//! only see DNS-level operations.
//!
//! Submodules:
//!
//! - [`cloudflare`] — Cloudflare v4 REST API, behind the
//!   `cloudflare` Cargo feature.
//!
//! `MockDnsProvider` (an in-process hickory-server-backed impl for
//! integration tests) lives in `vane-testutil` so non-test builds
//! never link the hickory stack.

#[cfg(feature = "cloudflare")]
pub mod cloudflare;

#[cfg(feature = "cloudflare")]
pub use cloudflare::{CloudflareConfig, CloudflareDnsProvider};

use std::time::Duration;

use async_trait::async_trait;

/// Authoritative-DNS interface used by [`super::ManagedCertRegistry::issue_dns01`].
///
/// All three methods are async because every real implementation
/// performs network I/O. Implementations must be safe to share
/// across tokio tasks (`Send + Sync`); the registry holds an
/// `Arc<dyn DnsProvider>` so the per-issuance lifetime can outlive
/// the call site.
///
/// `Debug` is required so `RegistryError::Acme` chains carry
/// useful provider-side context when the orchestrator surfaces a
/// failure — no provider should hide the API host or the zone-id
/// it's operating on.
#[async_trait]
pub trait DnsProvider: Send + Sync + std::fmt::Debug {
	/// Set / replace a TXT record at `name` with `value`. Returns
	/// after the upstream API confirms the write — propagation is
	/// the caller's responsibility via [`Self::wait_propagated`].
	///
	/// `name` is the absolute FQDN the ACME server will query
	/// (`_acme-challenge.<sni>`), with no trailing dot.
	///
	/// # Errors
	///
	/// - [`DnsProviderError::Auth`]: token / credential rejected.
	/// - [`DnsProviderError::ZoneNotFound`]: `name` falls outside
	///   any zone the provider can write.
	/// - [`DnsProviderError::Api`]: any other upstream failure.
	async fn set_txt(&self, name: &str, value: &str) -> Result<(), DnsProviderError>;

	/// Remove the TXT record at `name`. Idempotent — no-op when
	/// the record is already gone, so a duplicate cleanup at the
	/// tail of an aborted issuance doesn't surface as an error.
	///
	/// # Errors
	///
	/// As [`Self::set_txt`].
	async fn delete_txt(&self, name: &str) -> Result<(), DnsProviderError>;

	/// Block until the TXT record at `name` is observable from the
	/// resolver pool the provider considers authoritative.
	///
	/// Implementations choose their own resolver pool: production
	/// providers (e.g. Cloudflare) query a small set of public
	/// recursive resolvers (typically `1.1.1.1`, `8.8.8.8`); the
	/// in-process mock queries its own server. The trait stays
	/// agnostic so callers don't need to know which resolvers each
	/// provider uses.
	///
	/// Returns `Ok(())` once `value` is present in the answer set.
	///
	/// # Errors
	///
	/// - [`DnsProviderError::PropagationTimeout`] when `timeout`
	///   elapses without the record being observed.
	/// - Other variants on transport failure.
	async fn wait_propagated(
		&self,
		name: &str,
		value: &str,
		timeout: Duration,
	) -> Result<(), DnsProviderError>;
}

/// Errors a [`DnsProvider`] surfaces. Categorised so the registry's
/// orchestrator can branch on auth-vs-propagation failures without
/// string-matching, and so operator-facing diagnostics
/// (`get_certs.last_error` in Stage 3) carry stable error kinds.
#[derive(Debug, thiserror::Error)]
pub enum DnsProviderError {
	#[error("dns api request failed: {0}")]
	Api(String),
	#[error("dns provider authentication failed (check api token / credentials)")]
	Auth,
	#[error("dns zone not found for {0}")]
	ZoneNotFound(String),
	#[error("dns propagation timeout for {0}")]
	PropagationTimeout(String),
	#[error("dns provider internal: {0}")]
	Internal(String),
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Confirm `DnsProvider` is dyn-compatible so the registry's
	/// `Arc<dyn DnsProvider>` shape compiles. Construction goes
	/// through a concrete `NoopProvider` to avoid mock infrastructure
	/// in this trait-only commit.
	#[derive(Debug)]
	struct NoopProvider;

	#[async_trait]
	impl DnsProvider for NoopProvider {
		async fn set_txt(&self, _: &str, _: &str) -> Result<(), DnsProviderError> {
			Ok(())
		}
		async fn delete_txt(&self, _: &str) -> Result<(), DnsProviderError> {
			Ok(())
		}
		async fn wait_propagated(&self, _: &str, _: &str, _: Duration) -> Result<(), DnsProviderError> {
			Ok(())
		}
	}

	#[test]
	fn dns_provider_is_object_safe() {
		let _: Box<dyn DnsProvider> = Box::new(NoopProvider);
	}

	#[test]
	fn dns_provider_error_display_carries_context() {
		let zone = DnsProviderError::ZoneNotFound("example.com".to_owned());
		assert!(zone.to_string().contains("example.com"));
		let prop = DnsProviderError::PropagationTimeout("_acme-challenge.example.com".to_owned());
		assert!(prop.to_string().contains("_acme-challenge.example.com"));
		assert!(DnsProviderError::Auth.to_string().contains("authentication"));
		let api = DnsProviderError::Api("503 Service Unavailable".to_owned());
		assert!(api.to_string().contains("503"));
	}
}
