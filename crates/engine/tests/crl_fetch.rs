//! End-to-end coverage for the daemon-wide CRL cache.
//!
//! Fixtures: rcgen-built CA + issuer used to sign CRL DERs, plus a
//! hyper http1 server that serves the bytes from a shared `Mutex` so
//! tests can rotate the served payload mid-run. The
//! `DefaultCrlFetcher` follows the same hyper-rustls path as
//! production; these tests substitute a `StaticFetcher` for failure
//! injection (the URL fetch surface is exercised in
//! `cache_url_source_loads_via_default_fetcher`).
//!
//! See `spec/crates/engine-tls.md` § _CRL_ for the
//! authoritative semantics.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use rcgen::{
	CertificateParams, CertificateRevocationListParams, Issuer, KeyIdMethod, KeyPair,
	KeyUsagePurpose, RevocationReason, RevokedCertParams, SerialNumber,
};
use tokio::net::TcpListener;
use vane_engine::tls::{
	CrlCache, CrlError, CrlFetchFailure, CrlFetcher, CrlSourceId, DefaultCrlFetcher,
};

fn make_issuer() -> Issuer<'static, KeyPair> {
	let mut params = CertificateParams::new(vec!["fixture ca".into()]).expect("ca params");
	params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
	params.key_usages =
		vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::CrlSign];
	let key = KeyPair::generate().expect("ca key");
	Issuer::new(params, key)
}

fn build_crl(
	issuer: &Issuer<'_, KeyPair>,
	revoked_serials: &[u64],
	next_update_hours: i64,
) -> Vec<u8> {
	let now = time::OffsetDateTime::now_utc();
	let params = CertificateRevocationListParams {
		this_update: now,
		next_update: now + time::Duration::hours(next_update_hours),
		crl_number: SerialNumber::from(1u64),
		issuing_distribution_point: None,
		revoked_certs: revoked_serials
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

/// Spawn a minimal hyper http1 server that returns the bytes held by
/// `payload` for every request. Rotating `*payload.lock()` lets tests
/// flip the served CRL mid-run without restarting the server.
async fn spawn_crl_server(
	payload: Arc<Mutex<Vec<u8>>>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local_addr");
	let handle = tokio::spawn(async move {
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			let payload = Arc::clone(&payload);
			tokio::spawn(async move {
				let io = TokioIo::new(sock);
				let svc = service_fn(move |_req: hyper::Request<hyper::body::Incoming>| {
					let bytes = payload.lock().clone();
					async move {
						Ok::<_, Infallible>(
							hyper::Response::builder()
								.status(200)
								.header("content-type", "application/pkix-crl")
								.body(Full::new(Bytes::from(bytes)))
								.expect("response"),
						)
					}
				});
				let _ = hyper::server::conn::http1::Builder::new().serve_connection(io, svc).await;
			});
		}
	});
	(addr, handle)
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_url_source_loads_via_default_fetcher() {
	let issuer = make_issuer();
	let crl = build_crl(&issuer, &[100], 24);
	let payload = Arc::new(Mutex::new(crl));
	let (addr, _server) = spawn_crl_server(Arc::clone(&payload)).await;

	let fetcher = DefaultCrlFetcher::new_arc().expect("default fetcher");
	let cache = CrlCache::new(fetcher);
	let url = format!("http://{addr}/crl");
	cache
		.ensure_loaded(&[(CrlSourceId::Url(url.clone()), CrlFetchFailure::Tolerate)])
		.await
		.expect("ensure_loaded blocks and succeeds");
	let snap = cache.snapshot(std::slice::from_ref(&CrlSourceId::Url(url))).expect("snap ok");
	assert_eq!(snap.len(), 1, "snapshot returns one CRL");
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_reject_unreachable_url_fails_link() {
	// Bind + drop yields an address that's almost certainly closed.
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local_addr");
	drop(listener);
	let fetcher = DefaultCrlFetcher::new_arc().expect("default fetcher");
	let cache = CrlCache::new(fetcher);
	let url = format!("http://{addr}/crl");
	let err = cache
		.ensure_loaded(&[(CrlSourceId::Url(url), CrlFetchFailure::Reject)])
		.await
		.expect_err("reject must propagate");
	// Structured: the fetcher's HTTP connect failure surfaces as
	// `CrlError::Fetch` carrying the source identity.
	match err {
		CrlError::Fetch { src: CrlSourceId::Url(_), .. } => {}
		other => panic!("expected Fetch variant for Url source, got {other:?}"),
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_tolerate_unreachable_url_keeps_link_alive() {
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local_addr");
	drop(listener);
	let fetcher = DefaultCrlFetcher::new_arc().expect("default fetcher");
	let cache = CrlCache::new(fetcher);
	let url = format!("http://{addr}/crl");
	cache
		.ensure_loaded(&[(CrlSourceId::Url(url.clone()), CrlFetchFailure::Tolerate)])
		.await
		.expect("tolerate keeps link alive");
	let snap = cache.snapshot(std::slice::from_ref(&CrlSourceId::Url(url))).expect("snap ok");
	assert!(snap.is_empty(), "tolerate + never-loaded => silently dropped");
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_url_rotates_bytes_in_place_across_refresh() {
	let issuer = make_issuer();
	let crl_v1 = build_crl(&issuer, &[1], 24);
	let crl_v2 = build_crl(&issuer, &[1, 2, 3], 24);
	let payload = Arc::new(Mutex::new(crl_v1.clone()));
	let (addr, _server) = spawn_crl_server(Arc::clone(&payload)).await;

	let fetcher = DefaultCrlFetcher::new_arc().expect("default fetcher");
	let cache = CrlCache::new(fetcher);
	let src = CrlSourceId::Url(format!("http://{addr}/crl"));
	cache
		.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)])
		.await
		.expect("first ensure_loaded");
	let first = cache.snapshot(std::slice::from_ref(&src)).expect("first snapshot");
	assert_eq!(first.len(), 1);
	let first_arc = Arc::clone(&first[0]);

	// Rotate the served bytes and trigger a re-fetch via ensure_loaded.
	*payload.lock() = crl_v2.clone();
	cache
		.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)])
		.await
		.expect("second ensure_loaded");
	let second = cache.snapshot(std::slice::from_ref(&src)).expect("second snapshot");
	assert_eq!(second.len(), 1);
	assert_eq!(
		second[0].as_ref().as_ref(),
		crl_v2.as_slice(),
		"refresh installs new bytes via the same cache",
	);
	assert!(
		!Arc::ptr_eq(&first_arc, &second[0]),
		"new bytes get a new Arc — but the cache itself did not have to be rebuilt",
	);
}

/// A flipping fetcher that lets a single test transition the source's
/// health between `Healthy` and `Unavailable` without depending on a
/// real network race.
struct FlippingFetcher {
	bytes: Vec<u8>,
	succeed: AtomicBool,
}

#[async_trait]
impl CrlFetcher for FlippingFetcher {
	async fn fetch(&self, src: &CrlSourceId) -> Result<Vec<u8>, CrlError> {
		if self.succeed.load(Ordering::SeqCst) {
			Ok(self.bytes.clone())
		} else {
			Err(CrlError::fetch(src, "flip down"))
		}
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn tolerate_source_recovers_after_transient_outage() {
	let issuer = make_issuer();
	let crl = build_crl(&issuer, &[1], 24);
	let fetcher = Arc::new(FlippingFetcher { bytes: crl.clone(), succeed: AtomicBool::new(true) });
	let cache = CrlCache::new(Arc::clone(&fetcher) as Arc<dyn CrlFetcher>);
	let src = CrlSourceId::Url("https://crl.example/flip".into());
	cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)]).await.expect("initial load");

	// Simulate outage. ensure_loaded must still return Ok because the
	// policy is tolerate; the cached bytes remain available.
	fetcher.succeed.store(false, Ordering::SeqCst);
	cache
		.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)])
		.await
		.expect("tolerate during outage");
	let during = cache.snapshot(std::slice::from_ref(&src)).expect("snap during");
	assert_eq!(during.len(), 1, "stale bytes still served");

	// Recovery — the cache rotates back to Healthy on the next
	// successful fetch.
	fetcher.succeed.store(true, Ordering::SeqCst);
	cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)]).await.expect("recovery");
	let after = cache.snapshot(std::slice::from_ref(&src)).expect("snap after");
	assert_eq!(after.len(), 1);
}
