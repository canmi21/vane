//! Listener-side mTLS — `ClientTrustStore` loader and rustls
//! [`WebPkiClientVerifier`] wiring.
//!
//! Per `spec/architecture/08-tls.md` § _Client certificate verification
//! (mTLS on listener)_, the trust store is built from per-rule
//! `client_auth.trust_store` config: explicit PEM bundle paths
//! (`ca_paths`), an optional CA directory (`ca_dir`, all `*.pem` files
//! merged), and an optional list of CRL sources. This PR handles only
//! `kind: "file"` CRLs; URL CRL fetch lives with the daemon-wide CRL
//! cache (S3-11). The compile-time check for URL sources is in
//! `vane-core::compile::lower::compile_client_auth`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use rustls::RootCertStore;
use rustls::server::WebPkiClientVerifier;
use rustls_pki_types::CertificateRevocationListDer;
use vane_core::rule::{ClientAuthSpec, ClientTrustStoreConfig, CrlSourceConfig};

/// Loaded trust store ready for `WebPkiClientVerifier::builder(...)`.
/// CA roots are merged from every PEM source named in the config;
/// CRLs are owned-DER for use with `with_crls`.
///
/// Held inside `Arc<ArcSwap<_>>` per `08-tls.md` § _Cert resolver and
/// rotation_ — symmetric to `CertStore`. Reload swaps in a fresh
/// instance; live handshakes keep the trust store they captured at
/// handshake time.
pub struct ClientTrustStore {
	pub cas: Arc<RootCertStore>,
	pub crls: Vec<CertificateRevocationListDer<'static>>,
}

/// Stringly error returned by trust-store construction. The bind path
/// surfaces this back as `LinkError::ClientTrustStore`; the error
/// message names the file that failed.
#[derive(thiserror::Error, Debug)]
pub enum ClientTrustStoreError {
	#[error(
		"client trust store has no certs after loading {ca_paths:?} + {ca_dir:?} — supply at least one valid CA"
	)]
	Empty { ca_paths: Vec<PathBuf>, ca_dir: Option<PathBuf> },

	#[error("read ca_path {path:?}: {source}")]
	ReadCaPath { path: PathBuf, source: std::io::Error },

	#[error("parse ca_path {path:?}: {source}")]
	ParseCaPath { path: PathBuf, source: std::io::Error },

	#[error("ca_path {path:?} has no certs")]
	EmptyCaPath { path: PathBuf },

	#[error("read ca_dir {dir:?}: {source}")]
	ReadCaDir { dir: PathBuf, source: std::io::Error },

	#[error("read crl_path {path:?}: {source}")]
	ReadCrlPath { path: PathBuf, source: std::io::Error },

	#[error("parse crl_path {path:?}: {source}")]
	ParseCrlPath { path: PathBuf, source: std::io::Error },

	#[error("crl_path {path:?} has no CRLs")]
	EmptyCrlPath { path: PathBuf },

	#[error("rustls client verifier rejected the trust store: {source}")]
	BuildVerifier { source: rustls::server::VerifierBuilderError },
}

impl ClientTrustStore {
	/// Build a `ClientTrustStore` from the parsed config. Loads CAs
	/// from every `ca_paths` entry and every `*.pem` file in `ca_dir`,
	/// dedupes by full DER, then loads CRLs from `kind: "file"`
	/// sources only. URL sources should already have been rejected at
	/// compile time.
	///
	/// # Errors
	///
	/// Returns the [`ClientTrustStoreError`] variant naming the file or
	/// directory that failed to load / parse, or `Empty` when no CA
	/// material is reachable.
	pub fn from_config(cfg: &ClientTrustStoreConfig) -> Result<Self, ClientTrustStoreError> {
		let mut roots = RootCertStore::empty();
		let mut seen: HashSet<Vec<u8>> = HashSet::new();

		for path in &cfg.ca_paths {
			Self::add_pem_file(path, &mut roots, &mut seen)?;
		}
		if let Some(dir) = &cfg.ca_dir {
			Self::add_pem_dir(dir, &mut roots, &mut seen)?;
		}

		if roots.is_empty() {
			return Err(ClientTrustStoreError::Empty {
				ca_paths: cfg.ca_paths.clone(),
				ca_dir: cfg.ca_dir.clone(),
			});
		}

		let mut crls: Vec<CertificateRevocationListDer<'static>> = Vec::new();
		for src in &cfg.crls {
			match src {
				CrlSourceConfig::File { path, .. } => {
					Self::add_crl_file(path, &mut crls)?;
				}
				// TODO(s3-11): wire `Url` CRL sources through a
				// daemon-wide CRL cache. Today this branch is
				// unreachable — the lower pass rejects URL sources
				// at compile time.
				CrlSourceConfig::Url { .. } => unreachable!("URL CRL rejected at compile"),
			}
		}

		Ok(Self { cas: Arc::new(roots), crls })
	}

	fn add_pem_file(
		path: &Path,
		roots: &mut RootCertStore,
		seen: &mut HashSet<Vec<u8>>,
	) -> Result<(), ClientTrustStoreError> {
		let bytes = fs::read(path)
			.map_err(|source| ClientTrustStoreError::ReadCaPath { path: path.to_path_buf(), source })?;
		let mut reader = std::io::BufReader::new(bytes.as_slice());
		let mut count = 0_usize;
		for cert in rustls_pemfile::certs(&mut reader) {
			let cert = cert.map_err(|source| ClientTrustStoreError::ParseCaPath {
				path: path.to_path_buf(),
				source,
			})?;
			let der_bytes = cert.as_ref().to_vec();
			if seen.insert(der_bytes) {
				if let Err(e) = roots.add(cert) {
					return Err(ClientTrustStoreError::ParseCaPath {
						path: path.to_path_buf(),
						source: std::io::Error::other(e.to_string()),
					});
				}
				count += 1;
			}
		}
		if count == 0 {
			return Err(ClientTrustStoreError::EmptyCaPath { path: path.to_path_buf() });
		}
		Ok(())
	}

	fn add_pem_dir(
		dir: &Path,
		roots: &mut RootCertStore,
		seen: &mut HashSet<Vec<u8>>,
	) -> Result<(), ClientTrustStoreError> {
		let entries = fs::read_dir(dir)
			.map_err(|source| ClientTrustStoreError::ReadCaDir { dir: dir.to_path_buf(), source })?;
		for entry in entries {
			let entry = entry
				.map_err(|source| ClientTrustStoreError::ReadCaDir { dir: dir.to_path_buf(), source })?;
			let path = entry.path();
			if path.extension().and_then(|s| s.to_str()) != Some("pem") {
				continue;
			}
			Self::add_pem_file(&path, roots, seen)?;
		}
		Ok(())
	}

	fn add_crl_file(
		path: &Path,
		crls: &mut Vec<CertificateRevocationListDer<'static>>,
	) -> Result<(), ClientTrustStoreError> {
		let bytes = fs::read(path)
			.map_err(|source| ClientTrustStoreError::ReadCrlPath { path: path.to_path_buf(), source })?;
		let mut reader = std::io::BufReader::new(bytes.as_slice());
		let mut count = 0_usize;
		for crl in rustls_pemfile::crls(&mut reader) {
			let crl = crl.map_err(|source| ClientTrustStoreError::ParseCrlPath {
				path: path.to_path_buf(),
				source,
			})?;
			crls.push(crl);
			count += 1;
		}
		if count == 0 {
			return Err(ClientTrustStoreError::EmptyCrlPath { path: path.to_path_buf() });
		}
		Ok(())
	}
}

/// Build the rustls `Arc<dyn ClientCertVerifier>` for a listener whose
/// resolved `ClientAuthSpec` carries a `trust_store`. `Request` mode
/// adds `.allow_unauthenticated()` so handshakes succeed without a
/// cert; `Require` mode is strict.
///
/// Returns `None` for `ClientAuthSpec::None` — the caller drops back
/// to the existing `with_no_client_auth()` path.
///
/// # Errors
///
/// Surfaces any [`ClientTrustStoreError`] from trust-store
/// construction, plus rustls's own `BuildVerifier` rejection (e.g.
/// no roots after dedup, CRL signature verification failure).
pub fn build_client_verifier(
	spec: &ClientAuthSpec,
) -> Result<Option<Arc<dyn rustls::server::danger::ClientCertVerifier>>, ClientTrustStoreError> {
	let (allow_unauth, ts) = match spec {
		ClientAuthSpec::None => return Ok(None),
		ClientAuthSpec::Request { trust_store } => (true, trust_store),
		ClientAuthSpec::Require { trust_store } => (false, trust_store),
	};
	let store = ClientTrustStore::from_config(ts)?;
	let mut builder = WebPkiClientVerifier::builder(store.cas);
	if !store.crls.is_empty() {
		builder = builder.with_crls(store.crls);
	}
	if allow_unauth {
		builder = builder.allow_unauthenticated();
	}
	let verifier =
		builder.build().map_err(|source| ClientTrustStoreError::BuildVerifier { source })?;
	Ok(Some(verifier))
}

/// Per-listener handle wrapping the trust store in `ArcSwap` so future
/// reload diffs can rotate without reconstructing the listener's
/// `rustls::ServerConfig`. Each listener owns one of these alongside
/// its `CertStore` populator. For this PR the populator scope is
/// "load once at link time"; the swap path is wired but the trigger
/// is post-MVP (same shape as `CertStore`'s rotation).
pub struct ClientTrustStoreHandle {
	#[allow(
		dead_code,
		reason = "lifetime extension for future rotation; mirror of CertStore handle"
	)]
	pub store: Arc<ArcSwap<ClientTrustStore>>,
}

impl ClientTrustStoreHandle {
	#[must_use]
	pub fn new(store: ClientTrustStore) -> Self {
		Self { store: Arc::new(ArcSwap::from_pointee(store)) }
	}
}
