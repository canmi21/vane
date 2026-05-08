//! `DnsProvider` trait + error type for ACME DNS-01 challenges.
//!
//! The trait is intentionally narrow: a provider performs TXT-record
//! CRUD against an authoritative DNS API and confirms propagation.
//! ACME clients orchestrate the cleanup ordering, retry budget, and
//! finalization — providers only see DNS-level operations.
//!
//! Concrete implementations are gated behind feature flags so a
//! downstream build only pulls in transports for the providers it
//! actually uses. See the crate-level README for the table.

#[cfg(feature = "cloudflare")]
pub mod cloudflare;

use std::time::Duration;

use async_trait::async_trait;

/// Authoritative-DNS interface used by an ACME client to satisfy a
/// DNS-01 challenge.
///
/// All three methods are async because every real implementation
/// performs network I/O. Implementations must be safe to share
/// across tokio tasks (`Send + Sync`); ACME clients typically hold
/// an `Arc<dyn DnsProvider>` so the per-issuance lifetime can
/// outlive the call site.
///
/// `Debug` is required so error chains can carry useful provider-
/// side context — no provider should hide the API host or the
/// zone-id it's operating on.
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
	/// providers typically query a small set of public recursive
	/// resolvers (`1.1.1.1`, `8.8.8.8`); in-process mocks query
	/// their own server. The trait stays agnostic so callers don't
	/// need to know which resolvers each provider uses.
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

/// Errors a [`DnsProvider`] surfaces. Categorised so the orchestrator
/// can branch on auth-vs-propagation failures without string-
/// matching, and so operator-facing diagnostics carry stable error
/// kinds.
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
