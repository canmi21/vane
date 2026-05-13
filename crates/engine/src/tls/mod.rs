//! Listener-side TLS subsystem: cert store, SNI-aware resolver, and the
//! populator abstraction that feeds them.
//!
//! The store is held behind `Arc<arc_swap::ArcSwap<CertStore>>` so a
//! future rotation step can swap in a refreshed [`CertStore`] without
//! reconstructing `rustls::ServerConfig`. Live TLS connections keep
//! their handshake-time cert; only **new handshakes** see the new
//! store. See `spec/crates/engine-tls.md` § _Cert resolver_.

pub mod cert_store;
pub mod client_trust;
pub mod crl_cache;
pub mod populator;
pub mod resolver;
pub mod static_populator;

pub use cert_store::{CertEntry, CertStore};
pub use client_trust::{
	ClientTrustStore, ClientTrustStoreError, ClientTrustStoreHandle, build_client_verifier,
};
pub use crl_cache::{
	CrlCache, CrlError, CrlFetchFailure, CrlFetcher, CrlSourceId, DefaultCrlFetcher,
	collect_listener_crl_sources, collect_upstream_crl_sources, dedupe_crl_sources,
};
pub use ocsp_staple::{OcspError, OcspStaple};
pub use populator::{CertPopulator, PopulatorError};
pub use resolver::VaneCertResolver;
pub use rustls_crl_refresh::{RefreshableClientCertVerifier, RefreshableServerCertVerifier};
pub use rustls_native_roots_cache::{
	NativeRootsError, native_roots, refresh_native_roots, warm_native_roots,
};
#[cfg(feature = "aws-lc-rs")]
pub use rustls_ticketer::install_persistent_ticketer;
pub use rustls_ticketer::{default_ticketer, install_default_ticketer};
pub use static_populator::StaticCertPopulator;
