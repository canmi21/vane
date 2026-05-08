//! `CertPopulator` trait: the abstraction every cert source
//! implements. A populator owns "where certs come from" — disk PEM,
//! ACME, an enterprise PKI bridge — without leaking that choice into
//! the listener / resolver layer.
//!
//! Populators are **`FlowGraph`-scoped**: a fresh instance is
//! constructed on every [`crate::flow_graph::FlowGraph::link`]. Live
//! TLS connections are unaffected (the handshake-time cert is
//! captured), but populator in-memory state does not survive a reload.
//! Stateful populators (future `ManagedCertPopulator` for ACME / Let's
//! Encrypt) **must** persist their state via an on-disk cache so
//! reload-from-disk is indistinguishable from cold-start; otherwise
//! reload churn will exhaust upstream rate limits — Let's Encrypt
//! caps duplicate certificates at 5 per registered domain per week,
//! which is reached after 6 reloads in a worst case.
//! [`crate::tls::StaticCertPopulator`] is stateless and exempt.
//!
//! See `spec/crates/engine-tls.md` § _Cert populators / Populator
//! lifecycle_.

// TODO(populator-disk-storage): require a disk-backed Storage trait on
// stateful populators — see `spec/crates/engine-tls.md` § _Cert populators_.

use async_trait::async_trait;

use crate::tls::CertStore;

/// Errors a populator may surface during initial load or refresh.
#[derive(thiserror::Error, Debug)]
pub enum PopulatorError {
	/// PEM read / parse failure, ACME directory rejection, malformed
	/// cert chain, etc. The `String` carries a one-line diagnostic
	/// suitable for surfacing through `LinkError::TlsConfig`.
	#[error("{0}")]
	Source(String),
}

impl PopulatorError {
	pub(crate) fn source(msg: impl Into<String>) -> Self {
		Self::Source(msg.into())
	}
}

#[async_trait]
pub trait CertPopulator: Send + Sync {
	/// Build the initial [`CertStore`] for the listener this populator
	/// belongs to. Called once during `FlowGraph::link`.
	async fn initial_store(&self) -> Result<CertStore, PopulatorError>;

	/// Decide whether `current` is stale (near-expiry cert, expired
	/// OCSP staple, …) and, if so, return a fresh store for the
	/// listener's `ArcSwap` to install. `Ok(None)` means "still
	/// fresh, no swap needed".
	async fn refresh(&self, current: &CertStore) -> Result<Option<CertStore>, PopulatorError>;
}
