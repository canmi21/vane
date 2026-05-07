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

pub mod fs_store;
pub mod store;

pub use fs_store::FsAcmeStore;
pub use store::{AcmeAccount, AcmeStore, StoreError, StoredCert};
