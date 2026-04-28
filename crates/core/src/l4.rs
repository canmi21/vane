use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
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
	/// Physical listener socket — vane-owned, shared via `Arc` with the
	/// listener's recv loop. The fetch sends responses back to the peer
	/// through this socket; the listener demuxes inbound datagrams to
	/// the per-session forwarder via the dispatch table. See
	/// `spec/architecture/06-l4.md` § _UDP socket multiplexing_.
	pub socket: Arc<UdpSocket>,
	pub peer: SocketAddr,
	/// Datagram that triggered the cold-path `FlowGraph` entry. The
	/// `L4Forward` fetch sends this verbatim as the upstream session's
	/// first packet so no inbound bytes are lost between dispatch
	/// table miss and forwarder registration.
	pub first_packet: Bytes,
	/// `None` on the cold-path entry; populated only when an existing
	/// QUIC session takes over (post-MVP — see
	/// `spec/architecture/06-l4.md` § _`udp_dispatch`_).
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
