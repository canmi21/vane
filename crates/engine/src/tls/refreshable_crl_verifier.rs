//! Wrapper certificate verifiers that build a fresh
//! `WebPkiClientVerifier` / `WebPkiServerVerifier` per handshake from
//! the latest CRL snapshot held in [`crate::tls::CrlCache`]. This is
//! the mechanism that satisfies the `Arc<ClientConfig>` /
//! `Arc<ServerConfig>` stability invariant from
//! `spec/crates/engine-tls.md` § _CRL_ — refreshing CRL
//! bytes does not invalidate cached configs (and therefore does not
//! churn the upstream client cache); the wrapper just sees the new
//! bytes the next time it consults the cache.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, RootCertStore, SignatureScheme};

use super::crl_cache::{CrlCache, CrlSourceId};

/// Listener-side wrapper that defers to a freshly-built
/// `WebPkiClientVerifier` per handshake against the latest CRL bytes
/// pulled from `cache`.
#[derive(Debug)]
pub struct RefreshableClientCertVerifier {
	cache: Arc<CrlCache>,
	sources: Vec<CrlSourceId>,
	cas: Arc<RootCertStore>,
	allow_unauthenticated: bool,
	root_hint_subjects: Vec<DistinguishedName>,
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
		Arc::new(Self { cache, sources, cas, allow_unauthenticated, root_hint_subjects })
	}

	fn build_inner(&self) -> Result<Arc<dyn ClientCertVerifier>, rustls::Error> {
		let crls = self
			.cache
			.snapshot(&self.sources)
			.map_err(|e| rustls::Error::General(format!("crl unavailable: {e}")))?;
		let mut builder = WebPkiClientVerifier::builder(Arc::clone(&self.cas));
		if !crls.is_empty() {
			let owned: Vec<rustls::pki_types::CertificateRevocationListDer<'static>> =
				crls.iter().map(|arc| (**arc).clone()).collect();
			builder = builder.with_crls(owned);
		}
		if self.allow_unauthenticated {
			builder = builder.allow_unauthenticated();
		}
		builder.build().map_err(|e| rustls::Error::General(format!("verifier build: {e}")))
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
		// The crypto provider is installed at daemon boot; reach for it
		// rather than holding an Arc here, mirroring the pattern used
		// by `NoVerify` in `fetch::upstream`.
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

/// Upstream-side counterpart. Constructs a fresh
/// `WebPkiServerVerifier` per handshake.
#[derive(Debug)]
pub struct RefreshableServerCertVerifier {
	cache: Arc<CrlCache>,
	sources: Vec<CrlSourceId>,
	cas: Arc<RootCertStore>,
}

impl RefreshableServerCertVerifier {
	#[must_use]
	pub fn new(
		cache: Arc<CrlCache>,
		sources: Vec<CrlSourceId>,
		cas: Arc<RootCertStore>,
	) -> Arc<Self> {
		Arc::new(Self { cache, sources, cas })
	}

	fn build_inner(&self) -> Result<Arc<dyn ServerCertVerifier>, rustls::Error> {
		let crls = self
			.cache
			.snapshot(&self.sources)
			.map_err(|e| rustls::Error::General(format!("crl unavailable: {e}")))?;
		let mut builder = rustls::client::WebPkiServerVerifier::builder(Arc::clone(&self.cas));
		if !crls.is_empty() {
			let owned: Vec<rustls::pki_types::CertificateRevocationListDer<'static>> =
				crls.iter().map(|arc| (**arc).clone()).collect();
			builder = builder.with_crls(owned);
		}
		let inner =
			builder.build().map_err(|e| rustls::Error::General(format!("verifier build: {e}")))?;
		Ok(inner as Arc<dyn ServerCertVerifier>)
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
	use crate::tls::crl_cache::{CrlCache, CrlFetchFailure, CrlFetcher, CrlSourceId};

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
		// No crls — building the inner verifier must succeed; we don't
		// run a full handshake here. The presence of any roots is the
		// only precondition rustls enforces.
		assert!(v.build_inner().is_ok());
		assert!(v.client_auth_mandatory());
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn client_verifier_propagates_reject_unavailable() {
		install_crypto_once();
		let (cas, _issuer) = ca_only_root_store();
		// reject + never-loaded source must surface as `rustls::Error`
		// at build_inner time; this is what fails a handshake.
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
		// Building the inner verifier with valid CRL DER must succeed.
		assert!(v.build_inner().is_ok());
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
