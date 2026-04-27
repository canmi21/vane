use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use rustls_pki_types::CertificateDer;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConnId(pub u64);

impl std::fmt::Display for ConnId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{:016x}", self.0)
	}
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
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
	pub peer_cert: Option<CertificateDer<'static>>,
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
		// compile error — the spec (08-tls.md) constrains accepted versions
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
