//! Public types for protocol detection results stored on
//! `ConnContext.user` by the listener-side peek prelude.
//!
//! See `spec/architecture/06-l4.md` § _Protocol detection_. The
//! detector functions themselves (which run rustls's `Acceptor` to
//! parse a `ClientHello`) live in `vane-engine` since rustls is an
//! engine-level dependency. Predicates and middleware in `vane-core`
//! reach the buffer + parsed fields through the types defined here.

use bytes::Bytes;

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
	pub alpn: Vec<Vec<u8>>,
	pub versions: Vec<u16>,
}

/// Maximum number of bytes the listener-side peek prelude accumulates
/// before declaring the connection's prefix `Unknown`. Mirrors
/// `spec/architecture/06-l4.md` § _Protocol detection_'s 8 KiB cap.
pub const MAX_PEEK_BYTES: usize = 8 * 1024;

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
		assert!(h.versions.is_empty());
	}

	#[test]
	fn max_peek_bytes_matches_spec() {
		assert_eq!(MAX_PEEK_BYTES, 8 * 1024);
	}
}
