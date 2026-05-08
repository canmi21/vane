use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

use bytes::Bytes;
use parking_lot::Mutex;
use rustls_pki_types::CertificateDer;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConnId(pub u64);

impl std::fmt::Display for ConnId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{:016x}", self.0)
	}
}

#[derive(
	Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
pub enum Transport {
	Tcp,
	Udp,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum HttpVersion {
	Http1_0,
	Http1_1,
	Http2,
	Http3,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum TlsVersion {
	Tls12,
	Tls13,
}

#[derive(Clone, Debug, Default)]
pub struct TlsInfo {
	pub sni: Option<String>,
	pub alpn: Option<Vec<u8>>,
	pub version: Option<TlsVersion>,
	pub peer_cert: Option<Arc<PeerCertificate>>,
	/// Whether the client's request arrived (in part or wholly) as
	/// TLS 1.3 0-RTT (early data). Set at handshake completion in the
	/// engine's `run_tls` from rustls's `is_early_data_accepted()`.
	/// The L7 executor consults this together with the matched rule's
	/// `allow_zero_rtt` to decide whether to short-circuit the request
	/// with a synthetic 425 Too Early. See
	/// `spec/crates/engine-tls.md` § _TLS 1.3 0-RTT (early data)_.
	pub zero_rtt_used: bool,
}

/// Verified client certificate captured at TLS handshake time, with
/// every predicate-readable field pre-extracted so the per-Check
/// dispatch is allocation-light. Built once by the engine's
/// post-handshake population (`run_tls`); the seven
/// `tls.peer_cert.*` predicates read pre-computed strings off this
/// struct rather than re-parsing the DER on every test.
///
/// `leaf_der` retains the raw DER bytes so future predicates (or a
/// post-MVP debug surface) can re-derive any field x509-parser
/// exposes; the seven currently-spec'd fields are pre-extracted.
///
/// All `String`-typed fields are byte-for-byte canonical: hex digests
/// are ASCII-lowercase; `serial` is hex (lowercase, no leading-zero
/// stripping). See `spec/crates/core.md` §
/// _Predicate_ for the canonical formats.
#[derive(Clone, Debug, Default)]
pub struct PeerCertificate {
	/// Raw leaf cert DER. Retained for future predicates that need
	/// fields not pre-extracted; current readers should use the
	/// pre-extracted scalar fields below.
	pub leaf_der: Bytes,
	pub subject_cn: Option<String>,
	pub san_dns: Vec<String>,
	pub fingerprint_sha256: String,
	pub spki_sha256: String,
	pub issuer_cn: Option<String>,
	pub serial: String,
}

impl PeerCertificate {
	/// Pre-extract every `tls.peer_cert.*` predicate-readable field
	/// from a raw leaf cert DER. Returns `None` when the bytes are
	/// not a parseable X.509v3 certificate; the caller treats that as
	/// "no verified peer cert" (sound-by-default per spec).
	#[must_use]
	pub fn from_der(leaf_der: &CertificateDer<'_>) -> Option<Self> {
		use sha2::{Digest, Sha256};
		use x509_parser::prelude::*;

		let bytes = leaf_der.as_ref();
		let (_, cert) = X509Certificate::from_der(bytes).ok()?;
		let tbs = &cert.tbs_certificate;

		let subject_cn = tbs
			.subject()
			.iter_common_name()
			.next()
			.and_then(|attr| attr.as_str().ok().map(ToString::to_string));
		let issuer_cn = tbs
			.issuer()
			.iter_common_name()
			.next()
			.and_then(|attr| attr.as_str().ok().map(ToString::to_string));

		// SAN dNSName entries — RFC 5280 §4.2.1.6. Other GeneralName
		// variants (URI, RFC822, etc.) are not exposed via this path
		// per the predicate-schema table.
		let mut san_dns: Vec<String> = Vec::new();
		if let Ok(Some(san_ext)) = tbs.subject_alternative_name() {
			for name in &san_ext.value.general_names {
				if let GeneralName::DNSName(d) = name {
					san_dns.push((*d).to_string());
				}
			}
		}

		let mut hasher = Sha256::new();
		hasher.update(bytes);
		let fingerprint_sha256 = hex_lower(&hasher.finalize());

		let spki_sha256 = {
			let spki_der = tbs.subject_pki.raw;
			let mut h = Sha256::new();
			h.update(spki_der);
			hex_lower(&h.finalize())
		};

		// Serial: x509-parser gives BigUint; canonicalise as
		// lowercase hex, big-endian, no leading-zero stripping (per
		// spec). `to_bytes_be` returns the minimal-length
		// representation; pad nothing — operators copy the value out
		// verbatim from `openssl x509 -serial` when matching.
		let serial = hex_lower(&tbs.serial.to_bytes_be());

		Some(Self {
			leaf_der: Bytes::copy_from_slice(bytes),
			subject_cn,
			san_dns,
			fingerprint_sha256,
			spki_sha256,
			issuer_cn,
			serial,
		})
	}
}

fn hex_lower(bytes: &[u8]) -> String {
	use std::fmt::Write as _;
	let mut s = String::with_capacity(bytes.len() * 2);
	for b in bytes {
		let _ = write!(s, "{b:02x}");
	}
	s
}

pub struct ConnContext {
	pub id: ConnId,
	pub remote: SocketAddr,
	pub local: SocketAddr,
	pub transport: Transport,
	pub entered_at: Instant,

	pub tls: Mutex<Option<TlsInfo>>,
	pub http_version: OnceLock<HttpVersion>,

	pub user: Mutex<http::Extensions>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn conn_id_display_pads_zero_to_sixteen_hex_digits() {
		let rendered = format!("{}", ConnId(0));
		assert_eq!(rendered, "0000000000000000");
		assert_eq!(rendered.len(), 16);
	}

	#[test]
	fn conn_id_display_is_lowercase_hex() {
		let rendered = format!("{}", ConnId(0x0bad_f00d_dead_beef));
		assert_eq!(rendered, "0badf00ddeadbeef");
		assert!(rendered.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
	}

	#[test]
	fn conn_id_display_zero_pads_small_values() {
		// non-zero top nibble would mean no left padding; a small value exercises
		// the {:016x} pad path explicitly.
		let rendered = format!("{}", ConnId(1));
		assert_eq!(rendered, "0000000000000001");
	}

	#[test]
	fn conn_id_display_renders_u64_max() {
		let rendered = format!("{}", ConnId(u64::MAX));
		assert_eq!(rendered, "ffffffffffffffff");
		assert_eq!(rendered.len(), 16);
	}

	#[test]
	fn conn_id_serde_round_trip() {
		let id = ConnId(0x1234_5678_9abc_def0);
		let encoded = serde_json::to_string(&id).expect("serialize");
		let decoded: ConnId = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, id);
	}

	#[test]
	fn tls_version_variants_are_exhaustive_at_two() {
		// Adding a TlsVersion variant without updating this arm would be a
		// compile error — the spec (spec/crates/engine-tls.md) constrains accepted versions
		// to 1.2 and 1.3 only.
		for v in [TlsVersion::Tls12, TlsVersion::Tls13] {
			let matched = match v {
				TlsVersion::Tls12 => "1.2",
				TlsVersion::Tls13 => "1.3",
			};
			assert!(!matched.is_empty());
		}
	}

	#[test]
	fn tls_version_serde_round_trip_per_variant() {
		for v in [TlsVersion::Tls12, TlsVersion::Tls13] {
			let encoded = serde_json::to_string(&v).expect("serialize");
			let decoded: TlsVersion = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, v);
		}
	}

	#[test]
	fn transport_serde_round_trip_per_variant() {
		for t in [Transport::Tcp, Transport::Udp] {
			let encoded = serde_json::to_string(&t).expect("serialize");
			let decoded: Transport = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, t);
		}
	}

	#[test]
	fn http_version_serde_round_trip_per_variant() {
		for v in [HttpVersion::Http1_0, HttpVersion::Http1_1, HttpVersion::Http2, HttpVersion::Http3] {
			let encoded = serde_json::to_string(&v).expect("serialize");
			let decoded: HttpVersion = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, v);
		}
	}
}
