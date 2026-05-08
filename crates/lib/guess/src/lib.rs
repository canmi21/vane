//! Wire-protocol classifier for TCP / TLS streams.
//!
//! Given the first bytes of a freshly accepted connection, [`classify`]
//! runs a detector cascade and reports one of:
//!
//! - [`DetectedProtocol::TlsClientHello`] — the prefix is a complete
//!   TLS `ClientHello` (parsed via rustls's `Acceptor`); SNI and ALPN
//!   are extracted into [`PeekResult::tls`].
//! - [`DetectedProtocol::Http2Preface`] — the bytes match the 24-byte
//!   HTTP/2 connection preface from RFC 7540 §3.5.
//! - [`DetectedProtocol::Http1`] — the bytes start with a known
//!   HTTP/1 method and the request line carries an `HTTP/1.0` or
//!   `HTTP/1.1` version marker.
//! - [`DetectedProtocol::Unknown`] — every detector ruled itself out.
//!
//! The cascade is three-state: a detector can also say "I'd be willing
//! to commit if I saw a few more bytes." When *any* detector returns
//! that, [`classify`] surfaces `detected = None` so the caller can
//! read more bytes (up to [`MAX_PEEK_BYTES`]) and call again. When
//! every detector has ruled itself out, the result is `Unknown` and
//! further reads cannot change the outcome.
//!
//! ## Types-only consumers
//!
//! The default `classify` feature pulls in `rustls` (for the TLS
//! parse) and `memchr` (for the HTTP/1 scan). Disable defaults to
//! get only the result types — useful when a downstream crate wants
//! to *describe* a peek without performing one:
//!
//! ```toml
//! guess = { version = "0.2", default-features = false }
//! ```

use bytes::Bytes;

/// Outcome of one peek-buffer classification. `buffer` is the bytes
/// that were classified (kept on the result so consumers can replay
/// them to a downstream decoder via, e.g., `peeked-stream`).
/// `detected` is `None` when at least one detector wants more bytes;
/// the caller should read more and call [`classify`] again.
#[derive(Clone, Debug)]
pub struct PeekResult {
	pub buffer: Bytes,
	pub detected: Option<DetectedProtocol>,
	pub tls: Option<TlsClientHello>,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum DetectedProtocol {
	TlsClientHello,
	Http1,
	Http2Preface,
	QuicInitial,
	Dns,
	Unknown,
}

#[derive(Clone, Debug, Default)]
pub struct TlsClientHello {
	pub sni: Option<String>,
	/// ALPN protocol IDs offered by the client in the `ClientHello`.
	pub alpn: Vec<Vec<u8>>,
}

/// Maximum number of bytes a peek prelude should accumulate before
/// declaring the connection's prefix `Unknown`. 8 KiB matches what
/// most servers can read in a single non-blocking syscall and
/// covers any realistic TLS `ClientHello` (with SNI + ALPN + GREASE).
pub const MAX_PEEK_BYTES: usize = 8 * 1024;

#[cfg(feature = "classify")]
const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// HTTP/1 request methods recognised by the HTTP/1 detector.
/// Matched as a case-sensitive `<METHOD> ` prefix on the peek
/// buffer.
#[cfg(feature = "classify")]
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
#[cfg(feature = "classify")]
const HTTP1_VERSION_PREFIX: &[u8] = b" HTTP/1.";

/// Run the detector cascade against the current peek buffer.
///
/// Returns `Some(DetectedProtocol::*)` for a definitive match,
/// `None` (in [`PeekResult::detected`]) when *some* detector is
/// willing to wait for more bytes (the caller should keep reading
/// until it hits [`MAX_PEEK_BYTES`] or the read times out), and
/// `Some(DetectedProtocol::Unknown)` when every detector has ruled
/// itself out — at that point further reads cannot change the
/// outcome.
#[cfg(feature = "classify")]
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

	// Every detector ruled itself out — the prefix is opaque to us.
	PeekResult { buffer, detected: Some(DetectedProtocol::Unknown), tls: None }
}

#[cfg(feature = "classify")]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum DetectorOutcome {
	Match,
	NeedMore,
	NoMatch,
}

#[cfg(feature = "classify")]
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

#[cfg(feature = "classify")]
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

#[cfg(feature = "classify")]
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
/// confirmed [`detect_tls`] returned a `Match` for the same bytes;
/// on the (theoretically unreachable) re-parse failure path we fall
/// back to an empty `TlsClientHello` rather than panic.
#[cfg(feature = "classify")]
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
	TlsClientHello { sni, alpn }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn peek_result_is_clone_send_sync_static() {
		fn assert_bounds<T: Clone + Send + Sync + 'static>() {}
		assert_bounds::<PeekResult>();
	}

	#[test]
	fn detected_protocol_variants_are_distinct() {
		let all = [
			DetectedProtocol::TlsClientHello,
			DetectedProtocol::Http1,
			DetectedProtocol::Http2Preface,
			DetectedProtocol::QuicInitial,
			DetectedProtocol::Dns,
			DetectedProtocol::Unknown,
		];
		for (i, a) in all.iter().enumerate() {
			for (j, b) in all.iter().enumerate() {
				assert_eq!(a == b, i == j);
			}
		}
	}

	#[test]
	fn tls_client_hello_default_is_empty() {
		let h = TlsClientHello::default();
		assert!(h.sni.is_none());
		assert!(h.alpn.is_empty());
	}

	#[test]
	fn max_peek_bytes_is_8k() {
		assert_eq!(MAX_PEEK_BYTES, 8 * 1024);
	}

	#[cfg(feature = "classify")]
	mod classify {
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
			// `G` is a prefix of `GET ` — caller should read more bytes.
			let r = classify_short(b"G");
			assert!(r.detected.is_none());
		}

		#[test]
		fn classify_http1_http_0_9_request_line_does_not_match_http1() {
			// `GET /\r\n` is a valid HTTP/0.9 request — no version
			// marker before `\r\n`. Detector must reject it cleanly.
			let r = classify_short(b"GET /\r\n");
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
			let mut bad = H2_PREFACE.to_vec();
			*bad.last_mut().expect("preface non-empty") = b'x';
			let r = classify(&bad);
			assert_eq!(r.detected, Some(DetectedProtocol::Unknown));
		}

		#[test]
		fn classify_tls_client_hello_matches_and_extracts_sni_alpn() {
			install_crypto();
			let bytes = build_client_hello_bytes("api.example.com", &[b"h2".to_vec()]);
			let r = classify(&bytes);
			assert_eq!(r.detected, Some(DetectedProtocol::TlsClientHello));
			let tls = r.tls.expect("tls hello populated");
			assert_eq!(tls.sni.as_deref(), Some("api.example.com"));
			assert!(tls.alpn.iter().any(|p| p == b"h2"), "alpn includes h2: {:?}", tls.alpn);
		}

		#[test]
		fn classify_tls_truncated_is_indeterminate() {
			install_crypto();
			let bytes = build_client_hello_bytes("api.example.com", &[b"h2".to_vec()]);
			let r = classify(&bytes[..6]);
			assert!(r.detected.is_none());
		}

		#[test]
		fn classify_tls_byte_then_garbage_falls_back_to_unknown() {
			let mut buf = vec![0x16u8];
			buf.extend(std::iter::repeat_n(0xFFu8, 64));
			let r = classify(&buf);
			assert_eq!(r.detected, Some(DetectedProtocol::Unknown));
		}

		#[test]
		fn classify_random_8kib_is_unknown() {
			let buf: Vec<u8> = (0..MAX_PEEK_BYTES).map(|i| u8::try_from(i & 0xFF).unwrap()).collect();
			let r = classify(&buf);
			assert_eq!(r.detected, Some(DetectedProtocol::Unknown));
		}

		#[test]
		fn h2_preface_constant_matches_spec() {
			assert_eq!(H2_PREFACE, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
			assert_eq!(H2_PREFACE.len(), 24);
		}

		fn install_crypto() {
			let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
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

		/// Synthesise a TLS `ClientHello` by running rustls's own
		/// client-side state machine and capturing the bytes it would
		/// write to a hypothetical socket.
		fn build_client_hello_bytes(server_name: &str, alpn: &[Vec<u8>]) -> Vec<u8> {
			use std::sync::Arc;

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
}
