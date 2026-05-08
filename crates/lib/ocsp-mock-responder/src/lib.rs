//! In-process mock OCSP responder for integration tests.
//!
//! [`MockOcspResponder::start`] spins up a hyper HTTP/1.1 server on
//! an ephemeral port. Incoming `application/ocsp-request` POSTs are
//! parsed for the cert's serial; the responder then assembles a
//! [`x509_ocsp::OcspResponse`] (carrying a [`x509_ocsp::BasicOcspResponse`])
//! whose `SingleResponse` mirrors what a real CA responder would
//! send for that cert ID. The configured "status" lets per-test
//! cases pin the response to `Good`, `Revoked`, or `TryLater`.
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
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use der::asn1::BitString;
use der::{Decode, Encode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use spki::AlgorithmIdentifierOwned;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::warn;
use x509_cert::Certificate;
use x509_ocsp::{
	BasicOcspResponse, CertId, CertStatus, OcspGeneralizedTime, OcspRequest, OcspResponse,
	ResponderId, SingleResponse, Version,
};

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
		let issuer = Certificate::from_der(issuer_cert_der)
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

	let Ok(req_decoded) = OcspRequest::from_der(&body) else {
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
		OcspMockStatus::Good { next_update_in } => build_signed_response(
			issuer,
			cert_id,
			CertStatus::Good(der::asn1::Null),
			now,
			Some(now + next_update_in),
		),
		OcspMockStatus::Revoked => {
			let revoked = CertStatus::Revoked(x509_ocsp::RevokedInfo {
				revocation_time: OcspGeneralizedTime::try_from(now).expect("now"),
				revocation_reason: None,
			});
			build_signed_response(issuer, cert_id, revoked, now, None)
		}
		OcspMockStatus::TryLater => OcspResponse::try_later().to_der().expect("encode"),
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
	status: CertStatus,
	this_update: SystemTime,
	next_update: Option<SystemTime>,
) -> Vec<u8> {
	let mut single = SingleResponse::new(
		cert_id,
		status,
		OcspGeneralizedTime::try_from(this_update).expect("this_update"),
	);
	if let Some(nu) = next_update {
		single = single.with_next_update(OcspGeneralizedTime::try_from(nu).expect("next_update"));
	}

	// `OcspResponseBuilder::sign(...)` requires a real signer.
	// We instead construct the BasicOcspResponse manually with a
	// placeholder signature — the builder's structural pieces aren't
	// reachable as a public API for the no-sign path.
	let tbs = x509_ocsp::ResponseData {
		version: Version::V1,
		responder_id: ResponderId::ByName(issuer.tbs_certificate.subject.clone()),
		produced_at: OcspGeneralizedTime::try_from(this_update).expect("produced_at"),
		responses: vec![single],
		response_extensions: None,
	};
	// Placeholder signature: 32 bytes of zeroes encoded as a
	// BitString. The algorithm OID matches sha256WithRSAEncryption
	// (1.2.840.113549.1.1.11) since it's a recognisable real OID;
	// the bytes are bogus but well-formed DER which is all the
	// parser checks.
	let signature = BitString::from_bytes(&[0u8; 32]).expect("BitString");
	let signature_algorithm = AlgorithmIdentifierOwned {
		oid: const_oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION,
		parameters: Some(der::Any::from(der::asn1::Null)),
	};
	let basic =
		BasicOcspResponse { tbs_response_data: tbs, signature_algorithm, signature, certs: None };
	OcspResponse::successful(basic).expect("successful").to_der().expect("encode")
}

/// Drain the request body into a single `Bytes`. Pulled out as a
/// helper so the dispatch site stays readable and the
/// `http_body_util::BodyExt` import lives in one place.
async fn collect_body(req: Request<Incoming>) -> Result<Bytes, hyper::Error> {
	let collected = req.into_body().collect().await?;
	Ok(collected.to_bytes())
}
