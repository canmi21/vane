//! Listener-side mTLS — `ClientTrustStore` loader and rustls
//! [`WebPkiClientVerifier`] wiring.
//!
//! Per `spec/crates/engine-tls.md` § _Client certificate verification
//! (mTLS on listener)_, the trust store is built from per-rule
//! `client_auth.trust_store` config: explicit PEM bundle paths
//! (`ca_paths`), an optional CA directory (`ca_dir`, all `*.pem` files
//! merged), and an optional list of CRL sources. CRL bytes themselves
//! are fetched and cached daemon-wide by [`crate::tls::CrlCache`]; this
//! module only retains the source identities and policies so the
//! refreshable verifier can pull a fresh snapshot per handshake.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use rustls::RootCertStore;
use vane_core::rule::{ClientAuthSpec, ClientTrustStoreConfig, CrlSourceConfig};

use crate::tls::crl_cache::{CrlCache, CrlFetchFailure, CrlSourceId};
use crate::tls::refreshable_crl_verifier::RefreshableClientCertVerifier;

/// Loaded trust store ready for `WebPkiClientVerifier::builder(...)`.
/// CA roots are merged from every PEM source named in the config; CRL
/// source identities and per-source `fetch_failure` policies are kept
/// here so the wrapper verifier can ask the daemon-wide cache for the
/// latest bytes per handshake (spec § _CRL checking_).
///
/// Held inside `Arc<ArcSwap<_>>` per `08-tls.md` § _Cert resolver and
/// rotation_ — symmetric to `CertStore`. Reload swaps in a fresh
/// instance; live handshakes keep the trust store they captured at
/// handshake time.
pub struct ClientTrustStore {
	pub cas: Arc<RootCertStore>,
	pub crls: Vec<(CrlSourceId, CrlFetchFailure)>,
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

	#[error("rustls client verifier rejected the trust store: {source}")]
	BuildVerifier { source: rustls::server::VerifierBuilderError },
}

impl ClientTrustStore {
	/// Build a `ClientTrustStore` from the parsed config. Loads CAs
	/// from every `ca_paths` entry and every `*.pem` file in `ca_dir`,
	/// dedupes by full DER. CRL **bytes** are not loaded here — the
	/// daemon-wide [`CrlCache`] owns them; this only records the source
	/// identity + per-source `fetch_failure` policy.
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

		let crls = cfg.crls.iter().map(crl_source_from_config).collect();

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
}

/// Translate a `CrlSourceConfig` (parsed from rule JSON) into the
/// `(CrlSourceId, CrlFetchFailure)` shape the cache and trust store
/// share. The `CrlFetchFailure` enums in `vane-core` and `vane-engine`
/// are structurally identical but live in separate crates; this is the
/// single mapping point.
#[must_use]
pub fn crl_source_from_config(cfg: &CrlSourceConfig) -> (CrlSourceId, CrlFetchFailure) {
	match cfg {
		CrlSourceConfig::File { path, fetch_failure } => {
			(CrlSourceId::File(path.clone()), map_fetch_failure(*fetch_failure))
		}
		CrlSourceConfig::Url { url, fetch_failure } => {
			(CrlSourceId::Url(url.clone()), map_fetch_failure(*fetch_failure))
		}
	}
}

fn map_fetch_failure(p: vane_core::rule::CrlFetchFailure) -> CrlFetchFailure {
	match p {
		vane_core::rule::CrlFetchFailure::Tolerate => CrlFetchFailure::Tolerate,
		vane_core::rule::CrlFetchFailure::Reject => CrlFetchFailure::Reject,
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
/// `crl_cache` is required when the resolved trust store carries any
/// CRL sources; if it is `None` and `cfg.crls` is non-empty, the
/// returned verifier still works but every handshake will fail with
/// "crl source not registered" (same fail-closed posture as a reject
/// source). Callers in production paths supply
/// [`crate::SecurityConfig::crl_cache`].
///
/// # Errors
///
/// Surfaces any [`ClientTrustStoreError`] from trust-store
/// construction, plus rustls's own `BuildVerifier` rejection (e.g.
/// no roots after dedup).
pub fn build_client_verifier(
	spec: &ClientAuthSpec,
	crl_cache: Option<&Arc<CrlCache>>,
) -> Result<Option<Arc<dyn rustls::server::danger::ClientCertVerifier>>, ClientTrustStoreError> {
	let (allow_unauth, ts) = match spec {
		ClientAuthSpec::None => return Ok(None),
		ClientAuthSpec::Request { trust_store } => (true, trust_store),
		ClientAuthSpec::Require { trust_store } => (false, trust_store),
	};
	let store = ClientTrustStore::from_config(ts)?;
	let cas = store.cas;
	if store.crls.is_empty() {
		// No CRLs — keep the original direct construction so behaviour
		// is unchanged for trust stores that don't opt into revocation.
		let mut builder = rustls::server::WebPkiClientVerifier::builder(cas);
		if allow_unauth {
			builder = builder.allow_unauthenticated();
		}
		let verifier =
			builder.build().map_err(|source| ClientTrustStoreError::BuildVerifier { source })?;
		return Ok(Some(verifier));
	}

	// CRL sources present — wrap so each handshake reads the latest
	// bytes from the daemon-wide cache.
	let cache = crl_cache.cloned().unwrap_or_else(|| {
		// Tests / fixtures that exercise the verifier without a real
		// daemon get a degenerate cache that always reports
		// `not registered`. Real daemons always provide one.
		CrlCache::new(Arc::new(NeverFetcher))
	});
	let sources: Vec<CrlSourceId> = store.crls.iter().map(|(id, _)| id.clone()).collect();
	let verifier = RefreshableClientCertVerifier::new(cache, sources, cas, allow_unauth);
	Ok(Some(verifier as Arc<dyn rustls::server::danger::ClientCertVerifier>))
}

/// Test-only stand-in fetcher used when [`build_client_verifier`] is
/// called without a real cache (defensive path for integration tests).
/// Real daemon code always provides `Some(cache)`.
struct NeverFetcher;

#[async_trait::async_trait]
impl crate::tls::CrlFetcher for NeverFetcher {
	async fn fetch(&self, _src: &crate::tls::CrlSourceId) -> Result<Vec<u8>, String> {
		Err("crl cache not configured".into())
	}
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
