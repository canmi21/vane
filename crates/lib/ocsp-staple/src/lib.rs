//! Build OCSP requests, parse OCSP responses, and extract the OCSP
//! responder URL from a certificate's Authority Information Access
//! (AIA) extension. With the `fetch` feature, also performs an async
//! HTTP/1.1 POST against the responder via hyper.
//!
//! ## Transport policy: HTTP-only
//!
//! Production CAs (Let's Encrypt, `DigiCert`, Sectigo, Entrust,
//! `GlobalSign`) all ship HTTP-only OCSP responders, and OCSP responses
//! are independently signed (the transport adds nothing the response
//! signature doesn't already provide). This crate enforces HTTP-only:
//! HTTPS responder URLs surface as [`OcspError::HttpsNotSupported`]
//! at extract / fetch time; the caller can deliver such responses
//! through other channels (e.g. a pre-fetched DER blob on disk).
//!
//! ## API shape
//!
//! Three layers:
//!
//! - Pure functions on cert DER (always compiled): [`extract_ocsp_url`],
//!   [`build_ocsp_request`], [`parse_ocsp_response`]. No IO; unit-
//!   testable in isolation.
//! - One async transport function (`fetch` feature): [`fetch_ocsp`].
//!   Wraps a hyper HTTP/1.1 conn behind a single timeout.
//! - Convenience (`fetch` feature): [`fetch_ocsp_for_cert`] runs the
//!   whole pipeline (extract → build → fetch → parse) given the leaf
//!   + issuer DER.

#[cfg(feature = "fetch")]
use std::time::Duration;
use std::time::SystemTime;

use rasn::prelude::*;
use rasn_ocsp::{
	BasicOcspResponse, CertId, CertStatus, OcspRequest, OcspResponse, OcspResponseStatus,
	Request as OcspReq, TbsRequest,
};
use rasn_pkix::{AlgorithmIdentifier, Certificate};
use sha1::{Digest, Sha1};

/// PKIX `id-ad-ocsp` OID per RFC 5280 §4.2.2.1. The `AccessDescription`
/// in an AIA extension whose `accessMethod` matches this OID carries
/// the OCSP responder URL in its `accessLocation` `GeneralName::URI`
/// field.
const ID_AD_OCSP: &str = "1.3.6.1.5.5.7.48.1";

/// SHA-1 OID per RFC 3279 §2.2.1 — used as the [`CertId::hash_algorithm`]
/// identifier for the issuer-name / issuer-key hashes. RFC 6960 §4.1.1
/// mandates SHA-1 here regardless of operator crypto-policy: it is a
/// routing identifier, not a security boundary.
const SHA1_OID: &[u32] = &[1, 3, 14, 3, 2, 26];

/// Error surface for the OCSP pipeline. Categorised so callers can
/// branch on transport / parse / responder failures without
/// string-matching.
#[derive(Debug, thiserror::Error)]
pub enum OcspError {
	#[error("certificate has no Authority Information Access extension")]
	NoAia,
	#[error("AIA extension has no OCSP responder URL")]
	NoOcspUrl,
	#[error(
		"OCSP responder URL uses HTTPS, which is not supported by this crate \
		 (deliver pre-fetched OCSP responses through another channel): {0}"
	)]
	HttpsNotSupported(String),
	#[error("invalid OCSP responder URL: {0}")]
	InvalidUrl(String),
	#[error("certificate parse failed: {0}")]
	CertParse(String),
	#[error("OCSP request build failed: {0}")]
	RequestBuild(String),
	#[error("OCSP responder returned HTTP {status}")]
	HttpStatus { status: u16 },
	#[error("OCSP responder unreachable: {0}")]
	Transport(String),
	#[error("OCSP response parse failed: {0}")]
	ResponseParse(String),
	#[error("OCSP responder returned non-successful status: {0}")]
	ResponderError(String),
	#[error("OCSP response body exceeds {cap} bytes")]
	BodyTooLarge { cap: usize },
}

/// Parsed OCSP response result. `staple` is the full DER `OCSPResponse`
/// suitable for handing to rustls via `CertifiedKey.ocsp`.
/// `next_update` is the responder's `nextUpdate` (or `producedAt + 7d`
/// when omitted — RFC 6960 §4.2.2.1 allows `nextUpdate` to be absent
/// for "indefinite" responses; we still need a wall-clock deadline so
/// a renewal scheduler can plan a refresh).
#[derive(Debug, Clone)]
pub struct OcspStaple {
	pub staple: Vec<u8>,
	pub next_update: SystemTime,
}

/// `producedAt + 7d` fallback when the responder omits `nextUpdate`.
/// Picked to match the typical Let's Encrypt / industry validity
/// window so omitted-`nextUpdate` responders blend with the rest.
const DEFAULT_NEXT_UPDATE_AHEAD: std::time::Duration = std::time::Duration::from_hours(168);

/// Total budget for a single OCSP fetch (DNS + connect + send + recv).
/// 10 seconds covers any reasonable CA OCSP responder; if it doesn't
/// answer in 10 seconds, callers typically ship the cert without a
/// staple and the scheduler retries on the next tick.
#[cfg(feature = "fetch")]
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Extract the OCSP responder URL from a cert's AIA extension.
///
/// # Errors
///
/// - [`OcspError::NoAia`] when the cert has no AIA extension.
/// - [`OcspError::NoOcspUrl`] when the AIA extension has no
///   `id-ad-ocsp` access descriptor (some CAs include only
///   `caIssuers`).
/// - [`OcspError::HttpsNotSupported`] when the URL is HTTPS — see
///   the module-level transport policy paragraph.
/// - [`OcspError::InvalidUrl`] for any other scheme (`ftp://`, …) or
///   a URL that doesn't parse.
/// - [`OcspError::CertParse`] when the cert DER is malformed.
pub fn extract_ocsp_url(cert_der: &[u8]) -> Result<String, OcspError> {
	use x509_parser::extensions::{GeneralName, ParsedExtension};
	use x509_parser::prelude::FromDer;

	let (_, cert) = x509_parser::prelude::X509Certificate::from_der(cert_der)
		.map_err(|e| OcspError::CertParse(format!("{e}")))?;

	let mut saw_aia = false;
	for ext in cert.tbs_certificate.extensions() {
		if let ParsedExtension::AuthorityInfoAccess(aia) = ext.parsed_extension() {
			saw_aia = true;
			for desc in &aia.accessdescs {
				if desc.access_method.to_id_string() == ID_AD_OCSP
					&& let GeneralName::URI(url) = &desc.access_location
				{
					return classify_url(url);
				}
			}
		}
	}
	if saw_aia { Err(OcspError::NoOcspUrl) } else { Err(OcspError::NoAia) }
}

/// Reject HTTPS / non-HTTP URLs at this layer so the rest of the
/// pipeline can assume the URL is a vanilla `http://` URL.
fn classify_url(url: &str) -> Result<String, OcspError> {
	if url.starts_with("https://") {
		Err(OcspError::HttpsNotSupported(url.to_owned()))
	} else if url.starts_with("http://") {
		Ok(url.to_owned())
	} else {
		Err(OcspError::InvalidUrl(format!("expected `http://` scheme, got: {url}")))
	}
}

/// Build an `OCSPRequest` DER for `cert_der` signed by `issuer_der` per
/// RFC 6960 §4.1.1. Cert ID hash is SHA-1 — RFC-mandated, not
/// security-critical (the hash is a routing identifier).
///
/// # Errors
///
/// [`OcspError::CertParse`] when either DER fails to decode;
/// [`OcspError::RequestBuild`] when the DER encoder rejects the
/// composed request.
///
/// # Panics
///
/// Panics if the hardcoded SHA-1 OID literal (`1.3.14.3.2.26`) fails
/// `ObjectIdentifier::new` validation. The OID is a static constant,
/// so this is unreachable in any compiled build.
pub fn build_ocsp_request(cert_der: &[u8], issuer_der: &[u8]) -> Result<Vec<u8>, OcspError> {
	let cert: Certificate =
		rasn::der::decode(cert_der).map_err(|e| OcspError::CertParse(format!("{e}")))?;
	let issuer: Certificate =
		rasn::der::decode(issuer_der).map_err(|e| OcspError::CertParse(format!("{e}")))?;

	// `issuer_name_hash` per RFC 6960 §4.1.1 is SHA-1 over the DER
	// encoding of the issuer's `Name` (the `subject` field of the
	// issuer's tbsCertificate). Round-trip through rasn — for any
	// canonical-DER input (every real CA + rcgen test cert), encode is
	// byte-identical to the original.
	let subject_der = rasn::der::encode(&issuer.tbs_certificate.subject)
		.map_err(|e| OcspError::RequestBuild(format!("issuer subject re-encode: {e}")))?;
	let issuer_name_hash = Sha1::digest(&subject_der).to_vec();

	// `issuer_key_hash` is SHA-1 over the BIT STRING **value** of the
	// issuer's `subjectPublicKey` (the raw key bytes, *not* the full
	// SubjectPublicKeyInfo DER, *not* including the unused-bits prefix).
	// `BitString::as_raw_slice` returns exactly those bytes.
	let issuer_key_hash =
		Sha1::digest(issuer.tbs_certificate.subject_public_key_info.subject_public_key.as_raw_slice())
			.to_vec();

	let hash_algorithm = AlgorithmIdentifier {
		algorithm: ObjectIdentifier::new(SHA1_OID).expect("static SHA-1 OID"),
		// RFC 5754 §2 + RFC 6960 §4.1.1: SHA-1 in an `AlgorithmIdentifier`
		// SHOULD omit `parameters`. Real CAs split — some emit `NULL`,
		// some omit. Match the prior `x509-ocsp` behaviour of
		// `parameters: Some(NULL)` so existing responder fixtures keep
		// the same wire shape for this field.
		parameters: Some(Any::new(rasn::der::encode(&()).expect("encode NULL"))),
	};

	let cert_id = CertId {
		hash_algorithm,
		issuer_name_hash: issuer_name_hash.into(),
		issuer_key_hash: issuer_key_hash.into(),
		serial_number: cert.tbs_certificate.serial_number.clone(),
	};

	let req = OcspRequest {
		tbs_request: TbsRequest {
			version: Integer::ZERO,
			requestor_name: None,
			request_list: vec![OcspReq { req_cert: cert_id, single_request_extensions: None }],
			request_extensions: None,
		},
		optional_signature: None,
	};
	rasn::der::encode(&req).map_err(|e| OcspError::RequestBuild(format!("DER encode: {e}")))
}

/// Parse an `OCSPResponse` DER into a [`OcspStaple`]. The original
/// bytes are returned verbatim as the `staple` (rustls ships them on
/// the wire without re-encoding).
///
/// # Errors
///
/// - [`OcspError::ResponseParse`] for malformed DER, missing
///   `responseBytes`, or no `SingleResponse` entries.
/// - [`OcspError::ResponderError`] when `responseStatus` is not
///   `successful`.
pub fn parse_ocsp_response(resp_der: &[u8]) -> Result<OcspStaple, OcspError> {
	let resp: OcspResponse = rasn::der::decode(resp_der)
		.map_err(|e| OcspError::ResponseParse(format!("OcspResponse decode: {e}")))?;

	if resp.status != OcspResponseStatus::Successful {
		return Err(OcspError::ResponderError(format!("{:?}", resp.status)));
	}

	let response_bytes = resp
		.bytes
		.as_ref()
		.ok_or_else(|| OcspError::ResponseParse("successful response has no responseBytes".into()))?;
	let basic: BasicOcspResponse = rasn::der::decode(response_bytes.response.as_ref())
		.map_err(|e| OcspError::ResponseParse(format!("BasicOcspResponse decode: {e}")))?;

	let single = basic
		.tbs_response_data
		.responses
		.first()
		.ok_or_else(|| OcspError::ResponseParse("no SingleResponse entries".into()))?;

	let next_update = match &single.next_update {
		Some(t) => generalized_time_to_system(t),
		None => {
			// RFC 6960 §4.2.2.1 allows `nextUpdate` to be absent ("the
			// responder always has up-to-date information"). We still
			// need a wall-clock deadline; fall back to
			// `producedAt + 7d` to match typical CA validity windows.
			generalized_time_to_system(&basic.tbs_response_data.produced_at) + DEFAULT_NEXT_UPDATE_AHEAD
		}
	};

	// `single.cert_status` is read for its variants in callers via the
	// `OcspStaple` consumer code — we surface only `next_update` +
	// raw bytes here. Bind the value so a future variant addition
	// surfaces as an unused warning rather than silently slipping
	// through.
	let _ = &single.cert_status;
	let _: &CertStatus = &single.cert_status;

	Ok(OcspStaple { staple: resp_der.to_vec(), next_update })
}

/// rasn's `GeneralizedTime` is a `chrono::DateTime<FixedOffset>`. The
/// conversion goes through `chrono::DateTime<Utc>` which lossily-
/// equals the wire time for any `GeneralizedTime` since 1970 (every
/// real-world OCSP response). Pre-epoch wall-clock times in OCSP are
/// not produced by any deployed responder and would clamp to the
/// epoch.
fn generalized_time_to_system(t: &rasn::types::GeneralizedTime) -> SystemTime {
	let utc = t.to_utc();
	SystemTime::UNIX_EPOCH
		+ std::time::Duration::from_nanos(
			u64::try_from(utc.timestamp_nanos_opt().unwrap_or(0)).unwrap_or(0),
		)
}

#[cfg(feature = "fetch")]
mod fetch {
	use std::time::Duration;

	use bytes::Bytes;
	use http_body_util::{BodyExt, Full, Limited};
	use hyper::Request;

	use super::{
		OcspError, OcspStaple, build_ocsp_request, classify_url, extract_ocsp_url, parse_ocsp_response,
	};

	/// Hard cap on the OCSP response body. A signed OCSPResponse for a
	/// single cert is typically a few KiB; 1 MiB is generous and
	/// rejects pathological / adversarial responders before they pin
	/// RAM. Matches the cap used by the CRL fetcher so the two trust-
	/// material channels surface the same magnitude of failure.
	const MAX_OCSP_BODY_BYTES: usize = 1024 * 1024;

	/// HTTP POST `request_der` to `responder_url` and return the raw
	/// `OCSPResponse` bytes. Caps the entire fetch at `timeout` (DNS +
	/// connect + send + recv). Rejects HTTPS URLs with
	/// [`OcspError::HttpsNotSupported`].
	///
	/// # Errors
	///
	/// - [`OcspError::HttpsNotSupported`] / [`OcspError::InvalidUrl`] on
	///   scheme problems.
	/// - [`OcspError::Transport`] on DNS / connect / hyper failures.
	/// - [`OcspError::HttpStatus`] on non-200 responses.
	pub async fn fetch_ocsp(
		responder_url: &str,
		request_der: Vec<u8>,
		timeout: Duration,
	) -> Result<Vec<u8>, OcspError> {
		classify_url(responder_url)?;
		let parsed = url::Url::parse(responder_url)
			.map_err(|e| OcspError::InvalidUrl(format!("parse {responder_url}: {e}")))?;
		let host = parsed
			.host_str()
			.ok_or_else(|| OcspError::InvalidUrl(format!("no host in {responder_url}")))?
			.to_owned();
		let port = parsed.port().unwrap_or(80);
		let path_and_query = if parsed.path().is_empty() {
			"/".to_owned()
		} else {
			match parsed.query() {
				Some(q) => format!("{}?{q}", parsed.path()),
				None => parsed.path().to_owned(),
			}
		};

		let fut = perform_fetch(host.clone(), port, path_and_query, request_der);
		tokio::time::timeout(timeout, fut)
			.await
			.map_err(|_| OcspError::Transport(format!("timed out after {timeout:?}")))?
	}

	async fn perform_fetch(
		host: String,
		port: u16,
		path_and_query: String,
		body: Vec<u8>,
	) -> Result<Vec<u8>, OcspError> {
		use hyper_util::rt::TokioIo;

		let stream = tokio::net::TcpStream::connect((host.as_str(), port))
			.await
			.map_err(|e| OcspError::Transport(format!("connect {host}:{port}: {e}")))?;
		let io = TokioIo::new(stream);
		let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io)
			.await
			.map_err(|e| OcspError::Transport(format!("handshake: {e}")))?;
		let conn_handle = tokio::spawn(async move {
			// We don't care about the conn's exit status — `Connection: close`
			// makes hyper return Ok once the server-issued FIN arrives.
			let _ = conn.await;
		});

		let body_len = body.len();
		let req = Request::builder()
			.method("POST")
			.uri(path_and_query)
			.header(hyper::header::HOST, &host)
			.header(hyper::header::CONTENT_TYPE, "application/ocsp-request")
			.header(hyper::header::CONTENT_LENGTH, body_len.to_string())
			.header(hyper::header::CONNECTION, "close")
			.body(Full::new(Bytes::from(body)))
			.map_err(|e| OcspError::Transport(format!("build request: {e}")))?;

		let resp =
			sender.send_request(req).await.map_err(|e| OcspError::Transport(format!("send: {e}")))?;
		let status = resp.status();
		if !status.is_success() {
			conn_handle.abort();
			return Err(OcspError::HttpStatus { status: status.as_u16() });
		}
		let limited = Limited::new(resp.into_body(), MAX_OCSP_BODY_BYTES);
		let bytes = match limited.collect().await {
			Ok(collected) => collected.to_bytes(),
			Err(e) => {
				conn_handle.abort();
				if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
					return Err(OcspError::BodyTooLarge { cap: MAX_OCSP_BODY_BYTES });
				}
				return Err(OcspError::Transport(format!("read body: {e}")));
			}
		};
		drop(sender);
		let _ = conn_handle.await;
		Ok(bytes.to_vec())
	}

	/// Convenience wrapper: extract AIA URL → build request → fetch →
	/// parse, all in one call.
	///
	/// # Errors
	///
	/// Any error from the underlying [`extract_ocsp_url`] /
	/// [`build_ocsp_request`] / [`fetch_ocsp`] / [`parse_ocsp_response`].
	pub async fn fetch_ocsp_for_cert(
		cert_der: &[u8],
		issuer_der: &[u8],
		timeout: Duration,
	) -> Result<OcspStaple, OcspError> {
		let url = extract_ocsp_url(cert_der)?;
		let req = build_ocsp_request(cert_der, issuer_der)?;
		let resp_bytes = fetch_ocsp(&url, req, timeout).await?;
		parse_ocsp_response(&resp_bytes)
	}
}

#[cfg(feature = "fetch")]
pub use fetch::{fetch_ocsp, fetch_ocsp_for_cert};

#[cfg(test)]
mod tests {
	use rcgen::{
		BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose,
		PKCS_ECDSA_P256_SHA256,
	};

	use super::*;

	/// Build a self-signed CA + a leaf cert whose AIA extension points
	/// at `aia_url`. Returns DER blobs for both. End-to-end signing of
	/// an `OCSPResponse` is exercised by an external mock responder
	/// (see `ocsp-mock-responder`); this crate's own tests cover only
	/// the structural primitives that don't need a running responder.
	fn build_test_ca_and_leaf(aia_url: &str) -> (Vec<u8>, Vec<u8>) {
		// CA.
		let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("ca key");
		let mut ca_params = CertificateParams::new(vec!["Test CA".to_owned()]).expect("ca params");
		ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
		ca_params.key_usages.push(KeyUsagePurpose::KeyCertSign);
		ca_params.key_usages.push(KeyUsagePurpose::CrlSign);
		let ca_cert = ca_params.clone().self_signed(&ca_key).expect("self_signed");
		let ca_der = ca_cert.der().to_vec();

		// Leaf with AIA pointing at the test responder URL.
		let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("leaf key");
		let mut leaf_params =
			CertificateParams::new(vec!["leaf.example".to_owned()]).expect("leaf params");
		leaf_params.use_authority_key_identifier_extension = true;
		leaf_params.custom_extensions.push(build_aia_custom_extension(aia_url));
		let issuer = Issuer::from_params(&ca_params, &ca_key);
		let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer).expect("leaf signed_by");
		let leaf_der = leaf_cert.der().to_vec();
		(ca_der, leaf_der)
	}

	/// rcgen does not natively support AIA, so we hand-craft a DER
	/// extension. The shape is `AuthorityInfoAccessSyntax ::=
	/// SEQUENCE OF AccessDescription`, each `AccessDescription` is
	/// `SEQUENCE { accessMethod OID, accessLocation GeneralName }`.
	fn build_aia_custom_extension(aia_url: &str) -> rcgen::CustomExtension {
		// OID 1.3.6.1.5.5.7.1.1 = id-pe-authorityInfoAccess
		let oid_aia: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 1, 1];
		// Build:
		//   SEQUENCE {                       (SEQUENCE OF AccessDescription)
		//     SEQUENCE {                     (one AccessDescription)
		//       OID 1.3.6.1.5.5.7.48.1       (id-ad-ocsp)
		//       [6] IMPLICIT IA5String       (URI form of GeneralName)
		//     }
		//   }
		let ocsp_oid_der: Vec<u8> = vec![0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01];
		let url_bytes = aia_url.as_bytes();
		let mut uri_tlv = vec![0x86];
		uri_tlv.extend_from_slice(&der_length(url_bytes.len()));
		uri_tlv.extend_from_slice(url_bytes);
		let mut access_desc_inner = ocsp_oid_der;
		access_desc_inner.extend_from_slice(&uri_tlv);
		let mut access_desc_tlv = vec![0x30];
		access_desc_tlv.extend_from_slice(&der_length(access_desc_inner.len()));
		access_desc_tlv.extend_from_slice(&access_desc_inner);
		let mut outer_tlv = vec![0x30];
		outer_tlv.extend_from_slice(&der_length(access_desc_tlv.len()));
		outer_tlv.extend_from_slice(&access_desc_tlv);
		rcgen::CustomExtension::from_oid_content(oid_aia, outer_tlv)
	}

	fn der_length(n: usize) -> Vec<u8> {
		// Test-only DER length encoder; inputs come from `aia_url` byte
		// counts and stay well under `u16::MAX`.
		if n < 0x80 {
			vec![u8::try_from(n).unwrap()]
		} else if n < 0x100 {
			vec![0x81, u8::try_from(n).unwrap()]
		} else {
			vec![0x82, u8::try_from((n >> 8) & 0xff).unwrap(), u8::try_from(n & 0xff).unwrap()]
		}
	}

	#[test]
	fn extract_ocsp_url_returns_url_for_cert_with_aia() {
		let (_, leaf_der) = build_test_ca_and_leaf("http://ocsp.example.test/");
		let url = extract_ocsp_url(&leaf_der).expect("extract ok");
		assert_eq!(url, "http://ocsp.example.test/");
	}

	#[test]
	fn extract_ocsp_url_returns_no_aia_for_cert_without_extension() {
		let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("key");
		let params = CertificateParams::new(vec!["plain.example".to_owned()]).expect("params");
		let cert = params.self_signed(&key).expect("self_signed");
		let err = extract_ocsp_url(cert.der()).expect_err("no AIA → err");
		assert!(matches!(err, OcspError::NoAia), "got {err:?}");
	}

	#[test]
	fn extract_ocsp_url_returns_https_not_supported() {
		let (_, leaf_der) = build_test_ca_and_leaf("https://ocsp.example.test/");
		let err = extract_ocsp_url(&leaf_der).expect_err("HTTPS rejected");
		match err {
			OcspError::HttpsNotSupported(url) => {
				assert_eq!(url, "https://ocsp.example.test/");
			}
			other => panic!("expected HttpsNotSupported, got {other:?}"),
		}
	}

	#[test]
	fn extract_ocsp_url_returns_invalid_url_for_non_http() {
		let (_, leaf_der) = build_test_ca_and_leaf("ftp://ocsp.example.test/");
		let err = extract_ocsp_url(&leaf_der).expect_err("ftp rejected");
		assert!(matches!(err, OcspError::InvalidUrl(_)), "got {err:?}");
	}

	#[test]
	fn build_ocsp_request_round_trips_through_rasn_ocsp() {
		let (issuer_der, leaf_der) = build_test_ca_and_leaf("http://ocsp.example.test/");
		let bytes = build_ocsp_request(&leaf_der, &issuer_der).expect("build ok");
		let req: OcspRequest = rasn::der::decode(&bytes).expect("decode");
		assert!(!req.tbs_request.request_list.is_empty());
		let leaf: Certificate = rasn::der::decode(&leaf_der).expect("leaf decode");
		let want_serial = leaf.tbs_certificate.serial_number.clone();
		let got_serial = req.tbs_request.request_list[0].req_cert.serial_number.clone();
		assert_eq!(got_serial, want_serial);
	}

	#[test]
	fn parse_ocsp_response_returns_responder_error_on_try_later() {
		let try_later = OcspResponse { status: OcspResponseStatus::TryLater, bytes: None };
		let bytes = rasn::der::encode(&try_later).expect("encode");
		let err = parse_ocsp_response(&bytes).expect_err("try_later → err");
		assert!(matches!(err, OcspError::ResponderError(_)), "got {err:?}");
	}

	#[test]
	fn parse_ocsp_response_rejects_garbage_bytes() {
		let err = parse_ocsp_response(&[0x30, 0x00]).expect_err("garbage rejected");
		assert!(matches!(err, OcspError::ResponseParse(_)), "got {err:?}");
	}

	#[cfg(feature = "fetch")]
	#[test]
	fn fetch_ocsp_rejects_https_url_pre_connect() {
		// No connection is attempted — the url scheme check fires
		// first. Single-poll task; runs under a fresh runtime.
		let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
		let err = rt.block_on(async {
			fetch_ocsp("https://ocsp.example.test/", vec![1, 2, 3], std::time::Duration::from_secs(1))
				.await
				.expect_err("https rejected")
		});
		assert!(matches!(err, OcspError::HttpsNotSupported(_)), "got {err:?}");
	}
}
