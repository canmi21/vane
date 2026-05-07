//! ACME issuance plumbing per `spec/acme.md`.
//!
//! Daemon-scoped state (accounts, pending challenges, issued certs)
//! lives in [`registry::ManagedCertRegistry`]; persistence is
//! abstracted by [`store::AcmeStore`] with a disk-backed
//! [`fs_store::FsAcmeStore`] default.
//!
//! Feature-gated behind `acme` — non-ACME builds compile this entire
//! tree out so a `--no-default-features --features aws-lc-rs` build
//! never pulls `instant-acme` / `rcgen` / `fs4` / `futures`.

pub mod dns;
pub mod fs_store;
pub mod populator;
pub mod registry;
pub mod scheduler;
pub mod store;

pub use dns::{DnsProvider, DnsProviderError};
pub use fs_store::FsAcmeStore;
pub use populator::ManagedCertPopulator;
pub use registry::{
	ChallengeKey, ManagedCertRegistry, PendingChallenge, RegistryError, RenewalScheduler,
};
pub use scheduler::{CertState, CertStatus, RenewalJob, RenewalPlan};
pub use store::{AcmeAccount, AcmeStore, LockGuard, StoreError, StoredCert};
