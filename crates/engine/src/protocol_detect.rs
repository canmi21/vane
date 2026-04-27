//! Listener-side protocol detection. The peek prelude in
//! [`crate::listener`] reads up to [`MAX_PEEK_BYTES`] from a freshly
//! accepted connection and feeds the prefix here; [`classify`] runs the
//! built-in detectors in priority order (TLS → HTTP/2 → HTTP/1) and
//! returns a [`PeekResult`] for the listener to attach to
//! `ConnContext.user`.
//!
//! See `spec/architecture/06-l4.md` § _Protocol detection_. UDP-only
//! detectors (`QuicInitial`, `Dns`) are reserved enum variants here;
//! their bodies are stubbed pending the UDP listener.

use bytes::Bytes;
use vane_core::{DetectedProtocol, PeekResult, TlsClientHello};

const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// HTTP/1 request methods recognised by the L1 detector. Matched as a
/// case-sensitive `<METHOD> ` prefix on the peek buffer.
const HTTP1_METHODS: &[&[u8]] = &[
	b"GET ",
	b"POST ",
	b"PUT ",
	b"DELETE ",
	b"HEAD ",
	b"OPTIONS ",
	b"PATCH ",
	b"CONNECT ",
	b"TRACE ",
];

/// HTTP/1 request-line version anchor — recognised as definitive when
/// the byte immediately after this is `'0'` or `'1'`.
const HTTP1_VERSION_PREFIX: &[u8] = b" HTTP/1.";

/// Run the detector cascade against the current peek buffer.
///
/// Returns `Some(DetectedProtocol::*)` for a definitive match,
/// `None` when *some* detector is willing to wait for more bytes
/// (the listener should keep reading until it hits [`MAX_PEEK_BYTES`]
/// or the read times out), and `Some(Unknown)` when every detector
/// has ruled itself out — at that point further reads cannot change
/// the outcome.
#[must_use]
pub fn classify(buf: &[u8]) -> PeekResult {
	let buffer = Bytes::copy_from_slice(buf);

	if buf.is_empty() {
		return PeekResult { buffer, detected: None, tls: None };
	}

	match detect_tls(buf) {
		DetectorOutcome::Match => {
			let tls = parse_client_hello(buf);
			return PeekResult {
				buffer,
				detected: Some(DetectedProtocol::TlsClientHello),
				tls: Some(tls),
			};
		}
		DetectorOutcome::NeedMore => {
			return PeekResult { buffer, detected: None, tls: None };
		}
		DetectorOutcome::NoMatch => {}
	}

	match detect_h2_preface(buf) {
		DetectorOutcome::Match => {
			return PeekResult { buffer, detected: Some(DetectedProtocol::Http2Preface), tls: None };
		}
		DetectorOutcome::NeedMore => {
			return PeekResult { buffer, detected: None, tls: None };
		}
		DetectorOutcome::NoMatch => {}
	}

	match detect_http1(buf) {
		DetectorOutcome::Match => {
			return PeekResult { buffer, detected: Some(DetectedProtocol::Http1), tls: None };
		}
		DetectorOutcome::NeedMore => {
			return PeekResult { buffer, detected: None, tls: None };
		}
		DetectorOutcome::NoMatch => {}
	}

	// Every detector ruled itself out — the prefix is opaque to us, so
	// the executor will route the connection through the L4 subgraph.
	PeekResult { buffer, detected: Some(DetectedProtocol::Unknown), tls: None }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum DetectorOutcome {
	Match,
	NeedMore,
	NoMatch,
}

fn detect_tls(buf: &[u8]) -> DetectorOutcome {
	if buf.first() != Some(&0x16) {
		return DetectorOutcome::NoMatch;
	}
	let mut acceptor = rustls::server::Acceptor::default();
	let mut input: &[u8] = buf;
	if acceptor.read_tls(&mut input).is_err() {
		return DetectorOutcome::NoMatch;
	}
	match acceptor.accept() {
		Ok(Some(_)) => DetectorOutcome::Match,
		Ok(None) => DetectorOutcome::NeedMore,
		Err(_) => DetectorOutcome::NoMatch,
	}
}

fn detect_h2_preface(buf: &[u8]) -> DetectorOutcome {
	if buf.len() >= H2_PREFACE.len() {
		return if buf.starts_with(H2_PREFACE) {
			DetectorOutcome::Match
		} else {
			DetectorOutcome::NoMatch
		};
	}
	if H2_PREFACE.starts_with(buf) { DetectorOutcome::NeedMore } else { DetectorOutcome::NoMatch }
}

fn detect_http1(buf: &[u8]) -> DetectorOutcome {
	let mut full_method_match = false;
	let mut prefix_of_method = false;
	for m in HTTP1_METHODS {
		if buf.starts_with(m) {
			full_method_match = true;
			break;
		}
		if buf.len() < m.len() && m.starts_with(buf) {
			prefix_of_method = true;
		}
	}
	if !full_method_match {
		return if prefix_of_method { DetectorOutcome::NeedMore } else { DetectorOutcome::NoMatch };
	}

	// Method+SP matched. Look ahead for ` HTTP/1.[01]`. A `\r\n` seen
	// before the version anchor means the request line ended without
	// a known HTTP/1 marker (HTTP/0.9 or junk) — no match.
	let cr_lf = memchr::memmem::find(buf, b"\r\n");
	let version_at = memchr::memmem::find(buf, HTTP1_VERSION_PREFIX);
	match (version_at, cr_lf) {
		(Some(v), Some(rn)) if rn < v => DetectorOutcome::NoMatch,
		(Some(v), _) => {
			let digit_idx = v + HTTP1_VERSION_PREFIX.len();
			match buf.get(digit_idx).copied() {
				Some(b'0' | b'1') => DetectorOutcome::Match,
				Some(_) => DetectorOutcome::NoMatch,
				None => DetectorOutcome::NeedMore,
			}
		}
		(None, Some(_)) => DetectorOutcome::NoMatch,
		(None, None) => DetectorOutcome::NeedMore,
	}
}

/// Parse a complete `ClientHello` out of `buf`. Caller has already
/// confirmed [`detect_tls`] returned [`DetectorOutcome::Match`] for the
/// same bytes; on the (theoretically unreachable) re-parse failure
/// path we fall back to an empty `TlsClientHello` rather than panic.
fn parse_client_hello(buf: &[u8]) -> TlsClientHello {
	let mut acceptor = rustls::server::Acceptor::default();
	let mut input: &[u8] = buf;
	if acceptor.read_tls(&mut input).is_err() {
		return TlsClientHello::default();
	}
	let Ok(Some(accepted)) = acceptor.accept() else {
		return TlsClientHello::default();
	};
	let hello = accepted.client_hello();
	let sni = hello.server_name().map(str::to_ascii_lowercase);
	let alpn: Vec<Vec<u8>> =
		hello.alpn().map_or_else(Vec::new, |it| it.map(<[u8]>::to_vec).collect());
	// `versions` is left empty: rustls 0.23's `ClientHello` accessor
	// surface (`server_name` / `signature_schemes` / `alpn` /
	// `cipher_suites`) does not expose the `supported_versions`
	// extension. Predicates that branch on it will land alongside an
	// upstream rustls accessor or a hand-rolled extension parser.
	// TODO(s2-tls-versions): populate when accessor lands.
	TlsClientHello { sni, alpn, versions: Vec::new() }
}

#[cfg(test)]
mod tests {
	use super::*;

	fn classify_short(s: &[u8]) -> PeekResult {
		classify(s)
	}

	#[test]
	fn classify_empty_buffer_is_indeterminate() {
		let r = classify(&[]);
		assert!(r.detected.is_none());
		assert!(r.tls.is_none());
		assert!(r.buffer.is_empty());
	}

	#[test]
	fn classify_http1_get_request_line_matches_http1() {
		let r = classify_short(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
		assert_eq!(r.detected, Some(DetectedProtocol::Http1));
		assert!(r.tls.is_none());
	}

	#[test]
	fn classify_http1_post_request_line_matches_http1() {
		let r = classify_short(b"POST /x HTTP/1.0\r\n");
		assert_eq!(r.detected, Some(DetectedProtocol::Http1));
	}

	#[test]
	fn classify_http1_partial_method_is_indeterminate() {
		// `G` is a prefix of `GET ` — listener should read more bytes.
		let r = classify_short(b"G");
		assert!(r.detected.is_none());
	}

	#[test]
	fn classify_http1_http_0_9_request_line_does_not_match_http1() {
		// `GET /\r\n` is a valid HTTP/0.9 request — no version marker
		// before `\r\n`. Detector must reject it cleanly.
		let r = classify_short(b"GET /\r\n");
		// All detectors said NoMatch → Unknown.
		assert_eq!(r.detected, Some(DetectedProtocol::Unknown));
	}

	#[test]
	fn classify_http1_unknown_method_is_unknown() {
		let r = classify_short(b"FOO /index HTTP/1.1\r\n");
		assert_eq!(r.detected, Some(DetectedProtocol::Unknown));
	}

	#[test]
	fn classify_http2_preface_exact_match() {
		let r = classify_short(H2_PREFACE);
		assert_eq!(r.detected, Some(DetectedProtocol::Http2Preface));
	}

	#[test]
	fn classify_http2_preface_partial_is_indeterminate() {
		let r = classify_short(b"PRI * HTTP/2.0\r\n");
		assert!(r.detected.is_none());
	}

	#[test]
	fn classify_http2_preface_close_but_wrong_byte_is_unknown() {
		// Same length as the preface, last byte differs.
		let mut bad = H2_PREFACE.to_vec();
		*bad.last_mut().expect("preface non-empty") = b'x';
		let r = classify(&bad);
		// `PRI ` is not a known HTTP/1 method, so HTTP/1 also says no.
		// All three detectors NoMatch → Unknown.
		assert_eq!(r.detected, Some(DetectedProtocol::Unknown));
	}

	#[test]
	fn classify_tls_client_hello_matches_and_extracts_sni_alpn() {
		// Build a minimal TLS 1.3 ClientHello via rustls itself — the
		// fixture is whatever rustls's client emits with our knobs.
		let bytes = build_client_hello_bytes("api.example.com", &[b"h2".to_vec()]);
		let r = classify(&bytes);
		assert_eq!(r.detected, Some(DetectedProtocol::TlsClientHello));
		let tls = r.tls.expect("tls hello populated");
		assert_eq!(tls.sni.as_deref(), Some("api.example.com"));
		assert!(tls.alpn.iter().any(|p| p == b"h2"), "alpn includes h2: {:?}", tls.alpn);
	}

	#[test]
	fn classify_tls_truncated_is_indeterminate() {
		let bytes = build_client_hello_bytes("api.example.com", &[b"h2".to_vec()]);
		// Take just the 5-byte TLS record header + a couple bytes of
		// fragment — far short of a full ClientHello.
		let r = classify(&bytes[..6]);
		assert!(r.detected.is_none());
	}

	#[test]
	fn classify_tls_byte_then_garbage_falls_back_to_unknown() {
		// Starts with `0x16` so detect_tls is engaged, but the rest is
		// nonsense. After enough bytes rustls returns Err → NoMatch;
		// the cascade then walks H2 / HTTP/1, both NoMatch → Unknown.
		let mut buf = vec![0x16u8];
		buf.extend(std::iter::repeat_n(0xFFu8, 64));
		let r = classify(&buf);
		assert_eq!(r.detected, Some(DetectedProtocol::Unknown));
	}

	#[test]
	fn classify_random_8kib_is_unknown() {
		// The brief's "Unknown" branch: nothing matches once the buffer
		// is full. Use deterministic non-textual bytes.
		let buf: Vec<u8> =
			(0..vane_core::MAX_PEEK_BYTES).map(|i| u8::try_from(i & 0xFF).expect("low byte")).collect();
		let r = classify(&buf);
		assert_eq!(r.detected, Some(DetectedProtocol::Unknown));
	}

	#[test]
	fn h2_preface_constant_matches_spec() {
		assert_eq!(H2_PREFACE, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
		assert_eq!(H2_PREFACE.len(), 24);
	}

	#[derive(Debug)]
	struct NoVerify;
	impl rustls::client::danger::ServerCertVerifier for NoVerify {
		fn verify_server_cert(
			&self,
			_end_entity: &rustls::pki_types::CertificateDer<'_>,
			_intermediates: &[rustls::pki_types::CertificateDer<'_>],
			_server_name: &rustls::pki_types::ServerName<'_>,
			_ocsp_response: &[u8],
			_now: rustls::pki_types::UnixTime,
		) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
			Ok(rustls::client::danger::ServerCertVerified::assertion())
		}
		fn verify_tls12_signature(
			&self,
			_message: &[u8],
			_cert: &rustls::pki_types::CertificateDer<'_>,
			_dss: &rustls::DigitallySignedStruct,
		) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
			Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
		}
		fn verify_tls13_signature(
			&self,
			_message: &[u8],
			_cert: &rustls::pki_types::CertificateDer<'_>,
			_dss: &rustls::DigitallySignedStruct,
		) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
			Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
		}
		fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
			rustls::crypto::CryptoProvider::get_default()
				.expect("crypto provider")
				.signature_verification_algorithms
				.supported_schemes()
		}
	}

	/// Synthesise a TLS `ClientHello` by running rustls's own client-side
	/// state machine and capturing the bytes it would write to a
	/// hypothetical socket. Avoids hand-rolling record framing: the
	/// bytes we get back are real, the same ones a `rustls` client would
	/// put on the wire.
	fn build_client_hello_bytes(server_name: &str, alpn: &[Vec<u8>]) -> Vec<u8> {
		use std::sync::Arc;

		crate::crypto::install_default_provider();
		let mut config = rustls::ClientConfig::builder()
			.dangerous()
			.with_custom_certificate_verifier(Arc::new(NoVerify))
			.with_no_client_auth();
		config.alpn_protocols = alpn.to_vec();
		let server =
			rustls::pki_types::ServerName::try_from(server_name.to_owned()).expect("server name");
		let mut conn = rustls::ClientConnection::new(Arc::new(config), server).expect("client conn");
		let mut out = Vec::new();
		conn.write_tls(&mut out).expect("write_tls");
		out
	}
}
