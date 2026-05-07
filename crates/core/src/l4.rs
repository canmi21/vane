use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use tokio::net::{TcpStream, UdpSocket};

use crate::fetch::AsyncReadWrite;

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
	/// `spec/crates/engine-tls.md` § _TLS termination (L4 → L7
	/// upgrade)_.
	Tls(Box<dyn AsyncReadWrite + Send>),
	Udp(UdpAssoc),
}

pub struct UdpAssoc {
	/// Physical listener socket — vane-owned, shared via `Arc` with the
	/// listener's recv loop. The fetch sends responses back to the peer
	/// through this socket; the listener demuxes inbound datagrams to
	/// the per-session forwarder via the dispatch table. See
	/// `spec/crates/engine.md` § _UDP socket multiplexing_.
	pub socket: Arc<UdpSocket>,
	pub peer: SocketAddr,
	/// Datagrams that triggered the cold-path `FlowGraph` entry, in
	/// arrival order. Length is `1` for the immediate cold-path; `> 1`
	/// only when the listener went through the pending-peek state
	/// machine and the buffered datagrams replay together (per
	/// `spec/crates/engine.md` § _Multi-packet peek_ § _Replay to
	/// handler_). The `L4Forward` fetch sends every entry verbatim, in
	/// this order, before subscribing to the inbound hot-path channel.
	pub first_packets: Vec<Bytes>,
}

#[cfg(test)]
mod tests {
	use super::*;

	// Compile-gate: if L4Conn's variant shape changes, this signature fails
	// to type-check. No runtime assertion is warranted.
	fn _accepts_l4_conn(_: &L4Conn) {}
}
