//! Wrapper certificate verifiers that build a fresh
//! `WebPkiClientVerifier` / `WebPkiServerVerifier` per handshake from
//! the latest CRL snapshot held in [`crate::CrlCache`]. This is the
//! mechanism that satisfies the `Arc<ClientConfig>` /
//! `Arc<ServerConfig>` stability invariant: refreshing CRL bytes does
//! not invalidate cached configs, and the wrapper just sees the new
//! bytes the next time it consults the cache.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, CertificateRevocationListDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, RootCertStore, SignatureScheme};

use crate::cache::{CrlCache, CrlSourceId};

/// Identity fingerprint of a CRL snapshot. We hash by the
/// `Arc::as_ptr` of each `CertificateRevocationListDer` rather than by
/// the bytes themselves: [`CrlCache`] guarantees that the inner Arc
/// changes iff the bytes change, so the pointer-tuple is a cheap and
/// correct "did this snapshot move?" key. Heavy CRLs that arrive every
/// few hours therefore don't pay a per-handshake byte compare.
type CrlFingerprint = Vec<usize>;

fn fingerprint(crls: &[Arc<CertificateRevocationListDer<'static>>]) -> CrlFingerprint {
	crls.iter().map(|arc| Arc::as_ptr(arc) as usize).collect()
}

fn build_owned(
	crls: &[Arc<CertificateRevocationListDer<'static>>],
) -> Vec<CertificateRevocationListDer<'static>> {
	crls.iter().map(|arc| (**arc).clone()).collect()
}

struct CachedClient {
	fingerprint: CrlFingerprint,
	verifier: Arc<dyn ClientCertVerifier>,
}

struct CachedServer {
	fingerprint: CrlFingerprint,
	verifier: Arc<dyn ServerCertVerifier>,
}

/// Listener-side wrapper that defers to a `WebPkiClientVerifier`
/// rebuilt only when the cached CRL snapshot's Arc identity changes,
/// against the latest CRL bytes pulled from the cache.
pub struct RefreshableClientCertVerifier {
	cache: Arc<CrlCache>,
	sources: Vec<CrlSourceId>,
	cas: Arc<RootCertStore>,
	allow_unauthenticated: bool,
	root_hint_subjects: Vec<DistinguishedName>,
	cached: ArcSwapOption<CachedClient>,
}

impl std::fmt::Debug for RefreshableClientCertVerifier {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("RefreshableClientCertVerifier")
			.field("sources", &self.sources)
			.field("allow_unauthenticated", &self.allow_unauthenticated)
			.finish_non_exhaustive()
	}
}

impl RefreshableClientCertVerifier {
	#[must_use]
	pub fn new(
		cache: Arc<CrlCache>,
		sources: Vec<CrlSourceId>,
		cas: Arc<RootCertStore>,
		allow_unauthenticated: bool,
	) -> Arc<Self> {
		let root_hint_subjects = cas.subjects();
		Arc::new(Self {
			cache,
			sources,
			cas,
			allow_unauthenticated,
			root_hint_subjects,
			cached: ArcSwapOption::from(None),
		})
	}

	fn build_inner(&self) -> Result<Arc<dyn ClientCertVerifier>, rustls::Error> {
		let crls = self
			.cache
			.snapshot(&self.sources)
			.map_err(|e| rustls::Error::General(format!("crl unavailable: {e}")))?;
		let fp = fingerprint(&crls);
		if let Some(hit) = self.cached.load_full()
			&& hit.fingerprint == fp
		{
			return Ok(Arc::clone(&hit.verifier));
		}
		let mut builder = WebPkiClientVerifier::builder(Arc::clone(&self.cas));
		if !crls.is_empty() {
			builder = builder.with_crls(build_owned(&crls));
		}
		if self.allow_unauthenticated {
			builder = builder.allow_unauthenticated();
		}
		let verifier =
			builder.build().map_err(|e| rustls::Error::General(format!("verifier build: {e}")))?;
		let verifier: Arc<dyn ClientCertVerifier> = verifier;
		self
			.cached
			.store(Some(Arc::new(CachedClient { fingerprint: fp, verifier: Arc::clone(&verifier) })));
		Ok(verifier)
	}
}

impl ClientCertVerifier for RefreshableClientCertVerifier {
	fn offer_client_auth(&self) -> bool {
		true
	}

	fn client_auth_mandatory(&self) -> bool {
		!self.allow_unauthenticated
	}

	fn root_hint_subjects(&self) -> &[DistinguishedName] {
		&self.root_hint_subjects
	}

	fn verify_client_cert(
		&self,
		end_entity: &CertificateDer<'_>,
		intermediates: &[CertificateDer<'_>],
		now: UnixTime,
	) -> Result<ClientCertVerified, rustls::Error> {
		self.build_inner()?.verify_client_cert(end_entity, intermediates, now)
	}

	fn verify_tls12_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		// Defer to the process-wide rustls crypto provider — must be
		// installed by the host before any handshake runs.
		rustls::crypto::verify_tls12_signature(
			message,
			cert,
			dss,
			&rustls::crypto::CryptoProvider::get_default()
				.expect("rustls crypto provider installed at boot")
				.signature_verification_algorithms,
		)
	}

	fn verify_tls13_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls13_signature(
			message,
			cert,
			dss,
			&rustls::crypto::CryptoProvider::get_default()
				.expect("rustls crypto provider installed at boot")
				.signature_verification_algorithms,
		)
	}

	fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
		rustls::crypto::CryptoProvider::get_default()
			.expect("rustls crypto provider installed at boot")
			.signature_verification_algorithms
			.supported_schemes()
	}
}

/// Upstream-side counterpart. Reuses a cached `WebPkiServerVerifier`
/// across handshakes when the CRL snapshot's Arc identity is
/// unchanged; rebuilds only after a refresh swaps the underlying
/// bytes.
pub struct RefreshableServerCertVerifier {
	cache: Arc<CrlCache>,
	sources: Vec<CrlSourceId>,
	cas: Arc<RootCertStore>,
	cached: ArcSwapOption<CachedServer>,
}

impl std::fmt::Debug for RefreshableServerCertVerifier {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("RefreshableServerCertVerifier")
			.field("sources", &self.sources)
			.finish_non_exhaustive()
	}
}

impl RefreshableServerCertVerifier {
	#[must_use]
	pub fn new(
		cache: Arc<CrlCache>,
		sources: Vec<CrlSourceId>,
		cas: Arc<RootCertStore>,
	) -> Arc<Self> {
		Arc::new(Self { cache, sources, cas, cached: ArcSwapOption::from(None) })
	}

	fn build_inner(&self) -> Result<Arc<dyn ServerCertVerifier>, rustls::Error> {
		let crls = self
			.cache
			.snapshot(&self.sources)
			.map_err(|e| rustls::Error::General(format!("crl unavailable: {e}")))?;
		let fp = fingerprint(&crls);
		if let Some(hit) = self.cached.load_full()
			&& hit.fingerprint == fp
		{
			return Ok(Arc::clone(&hit.verifier));
		}
		let mut builder = rustls::client::WebPkiServerVerifier::builder(Arc::clone(&self.cas));
		if !crls.is_empty() {
			builder = builder.with_crls(build_owned(&crls));
		}
		let inner =
			builder.build().map_err(|e| rustls::Error::General(format!("verifier build: {e}")))?;
		let verifier: Arc<dyn ServerCertVerifier> = inner;
		self
			.cached
			.store(Some(Arc::new(CachedServer { fingerprint: fp, verifier: Arc::clone(&verifier) })));
		Ok(verifier)
	}
}

impl ServerCertVerifier for RefreshableServerCertVerifier {
	fn verify_server_cert(
		&self,
		end_entity: &CertificateDer<'_>,
		intermediates: &[CertificateDer<'_>],
		server_name: &ServerName<'_>,
		ocsp_response: &[u8],
		now: UnixTime,
	) -> Result<ServerCertVerified, rustls::Error> {
		self.build_inner()?.verify_server_cert(
			end_entity,
			intermediates,
			server_name,
			ocsp_response,
			now,
		)
	}

	fn verify_tls12_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls12_signature(
			message,
			cert,
			dss,
			&rustls::crypto::CryptoProvider::get_default()
				.expect("rustls crypto provider installed at boot")
				.signature_verification_algorithms,
		)
	}

	fn verify_tls13_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls13_signature(
			message,
			cert,
			dss,
			&rustls::crypto::CryptoProvider::get_default()
				.expect("rustls crypto provider installed at boot")
				.signature_verification_algorithms,
		)
	}

	fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
		rustls::crypto::CryptoProvider::get_default()
			.expect("rustls crypto provider installed at boot")
			.signature_verification_algorithms
			.supported_schemes()
	}
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;
	use std::sync::atomic::{AtomicUsize, Ordering};

	use async_trait::async_trait;
	use rustls::RootCertStore;

	use super::*;
	use crate::cache::{CrlCache, CrlFetchFailure, CrlFetcher, CrlSourceId};

	struct StaticFetcher {
		bytes: Vec<u8>,
		count: AtomicUsize,
	}

	#[async_trait]
	impl CrlFetcher for StaticFetcher {
		async fn fetch(&self, _src: &CrlSourceId) -> Result<Vec<u8>, String> {
			self.count.fetch_add(1, Ordering::SeqCst);
			Ok(self.bytes.clone())
		}
	}

	struct FailingFetcher;

	#[async_trait]
	impl CrlFetcher for FailingFetcher {
		async fn fetch(&self, _src: &CrlSourceId) -> Result<Vec<u8>, String> {
			Err("test failure".into())
		}
	}

	fn install_crypto_once() {
		// Idempotent — `install_default` is best-effort and other tests
		// in this crate also call it. Ignore the result.
		let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
	}

	fn ca_only_root_store() -> (Arc<RootCertStore>, rcgen::Issuer<'static, rcgen::KeyPair>) {
		use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
		let mut params = CertificateParams::new(vec!["test ca".into()]).expect("ca params");
		params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
		params.key_usages = vec![
			KeyUsagePurpose::KeyCertSign,
			KeyUsagePurpose::DigitalSignature,
			KeyUsagePurpose::CrlSign,
		];
		let key = KeyPair::generate().expect("ca key");
		let cert = params.clone().self_signed(&key).expect("self-sign ca");
		let cert_der = cert.der().clone();
		let issuer = Issuer::new(params, key);
		let mut store = RootCertStore::empty();
		store.add(cert_der).expect("add ca");
		(Arc::new(store), issuer)
	}

	fn fixture_crl(issuer: &rcgen::Issuer<'_, rcgen::KeyPair>, revoked: &[u64]) -> Vec<u8> {
		use rcgen::{
			CertificateRevocationListParams, KeyIdMethod, RevocationReason, RevokedCertParams,
			SerialNumber,
		};
		let now = time::OffsetDateTime::now_utc();
		let params = CertificateRevocationListParams {
			this_update: now,
			next_update: now + time::Duration::hours(24),
			crl_number: SerialNumber::from(1u64),
			issuing_distribution_point: None,
			revoked_certs: revoked
				.iter()
				.map(|s| RevokedCertParams {
					serial_number: SerialNumber::from(*s),
					revocation_time: now,
					reason_code: Some(RevocationReason::KeyCompromise),
					invalidity_date: None,
				})
				.collect(),
			key_identifier_method: KeyIdMethod::Sha256,
		};
		params.signed_by(issuer).expect("sign crl").der().as_ref().to_vec()
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn client_verifier_builds_with_empty_cache_when_no_sources() {
		install_crypto_once();
		let (cas, _issuer) = ca_only_root_store();
		let fetcher = Arc::new(StaticFetcher { bytes: vec![], count: AtomicUsize::new(0) });
		let cache = CrlCache::new(fetcher);
		let v = RefreshableClientCertVerifier::new(cache, Vec::new(), cas, false);
		assert!(v.build_inner().is_ok());
		assert!(v.client_auth_mandatory());
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn client_verifier_propagates_reject_unavailable() {
		install_crypto_once();
		let (cas, _issuer) = ca_only_root_store();
		let cache = CrlCache::new(Arc::new(FailingFetcher));
		let src = CrlSourceId::Url("https://crl.example/down".into());
		let _ = cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Reject)]);
		let v = RefreshableClientCertVerifier::new(cache, vec![src], cas, false);
		let err = v.build_inner().expect_err("reject unavailable must fail");
		match err {
			rustls::Error::General(msg) => assert!(msg.contains("crl unavailable"), "{msg}"),
			other => panic!("unexpected: {other:?}"),
		}
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn server_verifier_builds_with_real_crl_bytes() {
		install_crypto_once();
		let (cas, issuer) = ca_only_root_store();
		let bytes = fixture_crl(&issuer, &[42]);
		let fetcher = Arc::new(StaticFetcher { bytes, count: AtomicUsize::new(0) });
		let cache = CrlCache::new(fetcher);
		let src = CrlSourceId::Url("https://crl.example/with-revoke".into());
		cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)]).expect("load");
		let v = RefreshableServerCertVerifier::new(cache, vec![src], cas);
		assert!(v.build_inner().is_ok());
	}

	// Test-only fetcher that serves a different byte string on the
	// second fetch so we can simulate a real CRL rotation without
	// rebuilding the cache from scratch.
	struct SwapFetcher {
		calls: AtomicUsize,
		first: Vec<u8>,
		second: Vec<u8>,
	}

	#[async_trait]
	impl CrlFetcher for SwapFetcher {
		async fn fetch(&self, _src: &CrlSourceId) -> Result<Vec<u8>, String> {
			let n = self.calls.fetch_add(1, Ordering::SeqCst);
			Ok(if n == 0 { self.first.clone() } else { self.second.clone() })
		}
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn client_verifier_reuses_cached_inner_until_crl_arc_changes() {
		install_crypto_once();
		let (cas, issuer) = ca_only_root_store();
		let bytes_v1 = fixture_crl(&issuer, &[1]);
		let bytes_v2 = fixture_crl(&issuer, &[1, 2]);
		let fetcher =
			Arc::new(SwapFetcher { calls: AtomicUsize::new(0), first: bytes_v1, second: bytes_v2 });
		let cache = CrlCache::new(fetcher);
		let src = CrlSourceId::Url("https://crl.example/cached".into());
		cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)]).expect("load v1");
		let v = RefreshableClientCertVerifier::new(cache.clone(), vec![src.clone()], cas, false);
		let a = v.build_inner().expect("build a");
		let b = v.build_inner().expect("build b");
		assert!(Arc::ptr_eq(&a, &b), "cache hit reuses the same inner verifier Arc");

		// Refresh: cache now serves the v2 bytes, so a new build is
		// required.
		cache.ensure_loaded(&[(src, CrlFetchFailure::Tolerate)]).expect("refresh to v2");
		let c = v.build_inner().expect("build c");
		assert!(!Arc::ptr_eq(&b, &c), "post-refresh forces a rebuild");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn allow_unauthenticated_flips_mandatory_flag() {
		install_crypto_once();
		let (cas, _issuer) = ca_only_root_store();
		let fetcher = Arc::new(StaticFetcher { bytes: vec![], count: AtomicUsize::new(0) });
		let cache = CrlCache::new(fetcher);
		let v_mandatory =
			RefreshableClientCertVerifier::new(Arc::clone(&cache), Vec::new(), Arc::clone(&cas), false);
		let v_request = RefreshableClientCertVerifier::new(cache, Vec::new(), cas, true);
		assert!(v_mandatory.client_auth_mandatory());
		assert!(!v_request.client_auth_mandatory());
	}
}
