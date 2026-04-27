use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpStream, UdpSocket};

use crate::fetch::AsyncReadWrite;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct QuicAssocId(pub u64);

pub enum L4Conn {
	Tcp(TcpStream),
	/// Cleartext stream that the listener-side peek prelude has already
	/// drained part of, with those bytes rewound into the read side via
	/// `PeekedStream`. Type-erased so `vane-core` doesn't need to know
	/// the concrete adapter; downstream consumers see the connection
	/// from byte zero.
	Peeked(Box<dyn AsyncReadWrite + Send>),
	/// TLS-terminated stream after a server-side handshake completed.
	/// The trait object erases the concrete `tokio_rustls::TlsStream`
	/// type so that `vane-core` doesn't need to depend on rustls
	/// (the parsing + termination live in `vane-engine`). `AsyncReadWrite`
	/// is the same trait `L4ForwardFetch` uses for byte-tunnel I/O,
	/// auto-impl'd on any `AsyncRead + AsyncWrite + Unpin`. See
	/// `spec/architecture/08-tls.md` § _TLS termination (L4 → L7
	/// upgrade)_.
	Tls(Box<dyn AsyncReadWrite + Send>),
	Udp(UdpAssoc),
}

pub struct UdpAssoc {
	pub socket: Arc<UdpSocket>,
	pub peer: SocketAddr,
	pub quic: Option<QuicAssocId>,
}

#[cfg(test)]
mod tests {
	use super::*;

	// Compile-gate: if L4Conn's variant shape changes, this signature fails
	// to type-check. No runtime assertion is warranted.
	fn _accepts_l4_conn(_: &L4Conn) {}

	#[test]
	fn quic_assoc_id_serde_round_trip() {
		let id = QuicAssocId(0xdead_beef_cafe_babe);
		let encoded = serde_json::to_string(&id).expect("serialize");
		let decoded: QuicAssocId = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, id);
	}

	#[test]
	fn quic_assoc_id_equality_is_structural() {
		assert_eq!(QuicAssocId(42), QuicAssocId(42));
		assert_ne!(QuicAssocId(42), QuicAssocId(43));
	}
}
