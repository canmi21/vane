//! Listener-side TLS subsystem: cert store, SNI-aware resolver, and the
//! populator abstraction that feeds them.
//!
//! The store is held behind `Arc<arc_swap::ArcSwap<CertStore>>` so a
//! future rotation step can swap in a refreshed [`CertStore`] without
//! reconstructing `rustls::ServerConfig`. Live TLS connections keep
//! their handshake-time cert; only **new handshakes** see the new
//! store. See `spec/architecture/08-tls.md` § _Cert resolver and
//! rotation_.

pub mod cert_store;
pub mod client_trust;
pub mod crl_cache;
pub mod native_roots;
pub mod populator;
pub mod refreshable_crl_verifier;
pub mod resolver;
pub mod static_populator;
pub mod ticketer;

pub use cert_store::{CertEntry, CertStore};
pub use client_trust::{
	ClientTrustStore, ClientTrustStoreError, ClientTrustStoreHandle, build_client_verifier,
};
pub use crl_cache::{CrlCache, CrlFetchFailure, CrlFetcher, CrlSourceId, DefaultCrlFetcher};
pub use native_roots::{NativeRootsError, native_roots, warm_native_roots};
pub use populator::{CertPopulator, PopulatorError};
pub use refreshable_crl_verifier::{RefreshableClientCertVerifier, RefreshableServerCertVerifier};
pub use resolver::VaneCertResolver;
pub use static_populator::StaticCertPopulator;
pub use ticketer::{default_ticketer, install_default_ticketer};
