//! In-process mock OCSP responder for integration tests.
//!
//! [`MockOcspResponder::start`] spins up a hyper HTTP/1.1 server on
//! an ephemeral port. Incoming `application/ocsp-request` POSTs are
//! parsed for the cert's serial; the responder then assembles a
//! [`rasn_ocsp::OcspResponse`] (carrying a
//! [`rasn_ocsp::BasicOcspResponse`]) whose `SingleResponse` mirrors
//! what a real CA responder would send for that cert ID. The
//! configured "status" lets per-test cases pin the response to
//! `Good`, `Revoked`, or `TryLater`.
//!
//! ## Signing posture
//!
//! The response carries a placeholder signature. Most OCSP-stapling
//! consumers (rustls's `CertifiedKey.ocsp` path; OCSP-aware client
//! verifiers that don't independently re-sign-check the staple)
//! treat the staple as opaque bytes — they don't validate the
//! signature. The mock therefore avoids pulling an asymmetric
//! crypto crate (rsa / p256) into its dep graph; tests that need
//! real signatures (e.g. an OCSP-validating client that re-checks
//! the responder's signature) need to extend this fixture.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use rasn::prelude::*;
use rasn_ocsp::{
	BasicOcspResponse, CertId, CertStatus, OcspRequest, OcspResponse, OcspResponseStatus,
	ResponderId, ResponseBytes, ResponseData, RevokedInfo, SingleResponse, Version,
};
use rasn_pkix::{AlgorithmIdentifier, Certificate};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::warn;

/// `id-pkix-ocsp-basic` OID per RFC 6960 §4.2.1 — the
/// [`ResponseBytes::type`] tag for the `BasicOcspResponse` payload
/// every CA responder ships.
const ID_PKIX_OCSP_BASIC_OID: &[u32] = &[1, 3, 6, 1, 5, 5, 7, 48, 1, 1];

/// `sha256WithRSAEncryption` OID per RFC 8017 §A.2 — used as the
/// placeholder `signatureAlgorithm` on the un-signed mock response.
/// The bytes the algorithm identifier nominally signs are stubbed
/// (`[0u8; 32]`); real OCSP-validating consumers reject this, but
/// every fixture caller in this workspace consumes the staple as
/// opaque bytes (see the module-level "Signing posture" paragraph).
const SHA256_WITH_RSA_ENCRYPTION_OID: &[u32] = &[1, 2, 840, 113_549, 1, 1, 11];

#[derive(Debug, thiserror::Error)]
pub enum MockOcspError {
	#[error("bind {addr}: {source}")]
	Bind { addr: SocketAddr, source: std::io::Error },
	#[error("issuer cert decode: {0}")]
	IssuerDecode(String),
}

/// Per-request response status the mock returns. Tests use
/// [`MockOcspResponder::set_status`] to switch between branches
/// without restarting the server.
#[derive(Debug, Clone)]
pub enum OcspMockStatus {
	/// Successful response, cert is `Good`. `next_update_in`
	/// configures the response's `nextUpdate` relative to "now",
	/// driving any consumer's refresh-window decision.
	Good { next_update_in: Duration },
	/// Successful response, cert is `Revoked`. Parsers happily
	/// return a `Revoked` `CertStatus`; it is up to the consumer to
	/// reject the handshake (out of scope for this fixture).
	Revoked,
	/// Non-successful response. Consumers' parsers typically surface
	/// this as a "responder error" or "try later" status; the cert
	/// usually ships without a staple.
	TryLater,
}

impl OcspMockStatus {
	#[must_use]
	pub fn good_for(next_update_in: Duration) -> Self {
		Self::Good { next_update_in }
	}
}

/// Live mock OCSP responder. Drop the value to stop the server
/// (the spawned task is cancelled via the `shutdown` oneshot).
pub struct MockOcspResponder {
	addr: SocketAddr,
	status: Arc<Mutex<OcspMockStatus>>,
	hits: Arc<std::sync::atomic::AtomicUsize>,
	shutdown: Option<oneshot::Sender<()>>,
	_join: tokio::task::JoinHandle<()>,
}

impl MockOcspResponder {
	/// Spawn the responder against `issuer_cert_der`. The cert is
	/// used for `ResponderId::ByName(...)` in the response (the
	/// responder name must be derivable from the issuer). Initial
	/// status defaults to `Good { next_update_in: 7d }`.
	///
	/// # Errors
	///
	/// [`MockOcspError::Bind`] when binding the ephemeral port
	/// fails; [`MockOcspError::IssuerDecode`] when `issuer_cert_der`
	/// isn't a valid X.509 cert.
	///
	/// # Panics
	///
	/// Panics if `127.0.0.1:0` fails to parse — which would
	/// indicate a broken stdlib (parsing this literal cannot fail).
	pub async fn start(issuer_cert_der: &[u8]) -> Result<Self, MockOcspError> {
		let issuer: Certificate = rasn::der::decode(issuer_cert_der)
			.map_err(|e| MockOcspError::IssuerDecode(format!("{e}")))?;

		let listener = TcpListener::bind("127.0.0.1:0")
			.await
			.map_err(|e| MockOcspError::Bind { addr: "127.0.0.1:0".parse().unwrap(), source: e })?;
		let addr = listener.local_addr().map_err(|e| MockOcspError::Bind {
			addr: listener.local_addr().unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap()),
			source: e,
		})?;

		let status =
			Arc::new(Mutex::new(OcspMockStatus::Good { next_update_in: Duration::from_hours(168) }));
		let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
		let (tx, rx) = oneshot::channel::<()>();
		let issuer = Arc::new(issuer);
		let join =
			tokio::spawn(serve_loop(listener, issuer, Arc::clone(&status), Arc::clone(&hits), rx));

		Ok(Self { addr, status, hits, shutdown: Some(tx), _join: join })
	}

	#[must_use]
	pub fn url(&self) -> String {
		format!("http://{}/ocsp", self.addr)
	}

	#[must_use]
	pub fn addr(&self) -> SocketAddr {
		self.addr
	}

	pub fn set_status(&self, status: OcspMockStatus) {
		*self.status.lock() = status;
	}

	/// Number of OCSP requests successfully processed since
	/// startup. Tests use this to verify a populator / scheduler
	/// actually reached out to the responder.
	#[must_use]
	pub fn hits(&self) -> usize {
		self.hits.load(std::sync::atomic::Ordering::SeqCst)
	}
}

impl Drop for MockOcspResponder {
	fn drop(&mut self) {
		// Best-effort shutdown — the join handle lives long enough
		// for tokio to clean up on test exit even if our oneshot
		// signal arrives after the runtime is already winding down.
		if let Some(tx) = self.shutdown.take() {
			let _ = tx.send(());
		}
	}
}

async fn serve_loop(
	listener: TcpListener,
	issuer: Arc<Certificate>,
	status: Arc<Mutex<OcspMockStatus>>,
	hits: Arc<std::sync::atomic::AtomicUsize>,
	shutdown: oneshot::Receiver<()>,
) {
	let mut shutdown = std::pin::pin!(shutdown);
	loop {
		tokio::select! {
			_ = &mut shutdown => return,
			accept = listener.accept() => {
				let Ok((stream, _)) = accept else { continue };
				let issuer = Arc::clone(&issuer);
				let status = Arc::clone(&status);
				let hits = Arc::clone(&hits);
				tokio::spawn(async move {
					serve_one(stream, issuer, status, hits).await;
				});
			}
		}
	}
}

async fn serve_one(
	stream: tokio::net::TcpStream,
	issuer: Arc<Certificate>,
	status: Arc<Mutex<OcspMockStatus>>,
	hits: Arc<std::sync::atomic::AtomicUsize>,
) {
	let io = TokioIo::new(stream);
	let svc = service_fn(move |req: Request<Incoming>| {
		let issuer = Arc::clone(&issuer);
		let status = Arc::clone(&status);
		let hits = Arc::clone(&hits);
		async move { Ok::<_, std::convert::Infallible>(handle(req, &issuer, &status, &hits).await) }
	});
	if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
		warn!(target: "ocsp_mock_responder", error = %e, "mock OCSP conn ended with error");
	}
}

async fn handle(
	req: Request<Incoming>,
	issuer: &Certificate,
	status: &Mutex<OcspMockStatus>,
	hits: &std::sync::atomic::AtomicUsize,
) -> Response<Full<Bytes>> {
	if req.method() != Method::POST {
		return Response::builder()
			.status(StatusCode::METHOD_NOT_ALLOWED)
			.body(Full::new(Bytes::new()))
			.expect("static");
	}

	let body = match collect_body(req).await {
		Ok(b) => b,
		Err(e) => {
			warn!(target: "ocsp_mock_responder", error = %e, "body read failed");
			return Response::builder()
				.status(StatusCode::BAD_REQUEST)
				.body(Full::new(Bytes::new()))
				.expect("static");
		}
	};

	let Ok(req_decoded) = rasn::der::decode::<OcspRequest>(&body) else {
		return Response::builder()
			.status(StatusCode::BAD_REQUEST)
			.body(Full::new(Bytes::new()))
			.expect("static");
	};
	let Some(req_cert) = req_decoded.tbs_request.request_list.first() else {
		return Response::builder()
			.status(StatusCode::BAD_REQUEST)
			.body(Full::new(Bytes::new()))
			.expect("static");
	};

	let now = SystemTime::now();
	let cert_id = req_cert.req_cert.clone();
	let resp_bytes = match status.lock().clone() {
		OcspMockStatus::Good { next_update_in } => {
			build_signed_response(issuer, cert_id, CertStatus::Good, now, Some(now + next_update_in))
		}
		OcspMockStatus::Revoked => {
			let revoked = CertStatus::Revoked(RevokedInfo {
				revocation_time: system_to_generalized_time(now),
				revocation_reason: None,
			});
			build_signed_response(issuer, cert_id, revoked, now, None)
		}
		OcspMockStatus::TryLater => {
			let resp = OcspResponse { status: OcspResponseStatus::TryLater, bytes: None };
			rasn::der::encode(&resp).expect("encode TryLater")
		}
	};

	hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
	Response::builder()
		.status(StatusCode::OK)
		.header(hyper::header::CONTENT_TYPE, "application/ocsp-response")
		.header(hyper::header::CONTENT_LENGTH, resp_bytes.len().to_string())
		.body(Full::from(resp_bytes))
		.expect("response build")
}

/// Build an `OCSPResponse` DER for `cert_id` against `issuer`. The
/// signature is a placeholder — see the module-level "Signing
/// posture" paragraph.
fn build_signed_response(
	issuer: &Certificate,
	cert_id: CertId,
	cert_status: CertStatus,
	this_update: SystemTime,
	next_update: Option<SystemTime>,
) -> Vec<u8> {
	let single = SingleResponse {
		cert_id,
		cert_status,
		this_update: system_to_generalized_time(this_update),
		next_update: next_update.map(system_to_generalized_time),
		single_extensions: None,
	};

	let tbs = ResponseData {
		version: Version::ZERO,
		responder_id: ResponderId::ByName(issuer.tbs_certificate.subject.clone()),
		produced_at: system_to_generalized_time(this_update),
		responses: vec![single],
		response_extensions: None,
	};

	// Placeholder signature: 32 bytes of zeroes encoded as a BIT
	// STRING. The algorithm OID matches `sha256WithRSAEncryption` so
	// it is a recognisable real OID; the actual bytes don't sign
	// anything, but every fixture consumer treats the staple as
	// opaque so this is fine.
	let signature = BitString::from_slice(&[0u8; 32]);
	let signature_algorithm = AlgorithmIdentifier {
		algorithm: ObjectIdentifier::new(SHA256_WITH_RSA_ENCRYPTION_OID).expect("static OID"),
		parameters: Some(Any::new(rasn::der::encode(&()).expect("encode NULL"))),
	};
	let basic =
		BasicOcspResponse { tbs_response_data: tbs, signature_algorithm, signature, certs: None };

	// Wrap as `OcspResponse { status: Successful, bytes: Some(...) }`
	// per RFC 6960 §4.2.1. The `id-pkix-ocsp-basic` OID tags the
	// inner `BasicOcspResponse` payload.
	let basic_der = rasn::der::encode(&basic).expect("encode BasicOcspResponse");
	let resp = OcspResponse {
		status: OcspResponseStatus::Successful,
		bytes: Some(ResponseBytes {
			r#type: ObjectIdentifier::new(ID_PKIX_OCSP_BASIC_OID).expect("static OID"),
			response: basic_der.into(),
		}),
	};
	rasn::der::encode(&resp).expect("encode OcspResponse")
}

/// Convert a `SystemTime` to rasn's `GeneralizedTime`
/// (`chrono::DateTime<FixedOffset>`). Pre-epoch inputs clamp to the
/// epoch — no deployed OCSP responder emits pre-1970 timestamps, so
/// this branch is unreachable in practice.
fn system_to_generalized_time(t: SystemTime) -> rasn::types::GeneralizedTime {
	let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
	let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs.try_into().unwrap_or(0), 0)
		.unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("epoch"));
	dt.fixed_offset()
}

/// Drain the request body into a single `Bytes`. Pulled out as a
/// helper so the dispatch site stays readable and the
/// `http_body_util::BodyExt` import lives in one place.
async fn collect_body(req: Request<Incoming>) -> Result<Bytes, hyper::Error> {
	let collected = req.into_body().collect().await?;
	Ok(collected.to_bytes())
}
