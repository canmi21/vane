//! HTTP/3 listener-side glue: per-listener [`VirtualUdpSocket`]
//! implementing [`quinn::AsyncUdpSocket`] against vane's owned physical
//! UDP socket, plus the per-listener [`quinn::Endpoint`] bring-up that
//! installs the daemon's [`crate::tls::VaneCertResolver`] for ALPN `h3`.
//!
//! See `spec/architecture/06-l4.md` § _UDP socket multiplexing: physical
//! and virtual_, and `spec/architecture/08-tls.md` § _Cert resolver and
//! rotation_. The whole module is gated behind the `h3` cargo feature.
//
// TODO(s3-01-followup): the spec discusses per-connection virtual
// sockets keyed by `ConnectionId`; this PR uses one virtual socket per
// listener and lets quinn::Endpoint demux connections internally. The
// design tradeoff is captured in `notes/s3-01-question-virtual-socket-model.md`
// (uncommitted). Future PRs that wire `quinn-proto`'s `NewIdentifiers`
// event stream into the listener can split this back out into per-CID
// dispatch.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use parking_lot::Mutex;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::net::UdpSocket;

/// Bounded inbound queue per virtual socket. Full = drop, mirroring
/// `listener_udp.rs::SESSION_INBOUND_CAPACITY` — the listener loop must
/// never stall on a single misbehaving connection.
pub const VIRTUAL_INBOUND_CAPACITY: usize = 256;

/// Per-listener wrapper that satisfies [`quinn::AsyncUdpSocket`] without
/// giving quinn exclusive ownership of vane's physical UDP socket.
///
/// Inbound: the listener's recv loop pushes datagrams onto `inbound`
/// (drop-on-full); `poll_recv` drains them. Outbound: `try_send`
/// forwards quinn's transmits to the physical socket via
/// `tokio::net::UdpSocket::try_send_to` (non-blocking; surfaces
/// `WouldBlock` for quinn's poller to retry).
///
/// One instance per UDP+`Http` listener. `quinn::Endpoint` demuxes
/// connections by `ConnectionId` internally over that single socket;
/// the dispatch-table layer above only needs to fan datagrams in and
/// out.
pub struct VirtualUdpSocket {
	physical: Arc<UdpSocket>,
	inbound: Mutex<Inbound>,
	closed: AtomicBool,
}

struct Inbound {
	queue: std::collections::VecDeque<(SocketAddr, Bytes)>,
	waker: Option<Waker>,
	capacity: usize,
}

impl fmt::Debug for VirtualUdpSocket {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("VirtualUdpSocket")
			.field("closed", &self.closed.load(Ordering::Relaxed))
			.finish_non_exhaustive()
	}
}

impl VirtualUdpSocket {
	#[must_use]
	pub fn new(physical: Arc<UdpSocket>) -> Arc<Self> {
		Arc::new(Self {
			physical,
			inbound: Mutex::new(Inbound {
				queue: std::collections::VecDeque::new(),
				waker: None,
				capacity: VIRTUAL_INBOUND_CAPACITY,
			}),
			closed: AtomicBool::new(false),
		})
	}

	/// Push `datagram` onto the inbound queue. Called from the
	/// listener recv loop's hot-path hit. Drops the datagram if the
	/// queue is full — UDP is lossy by design and back-pressure to the
	/// listener loop would block every other session sharing the
	/// physical socket.
	pub fn enqueue_inbound(&self, peer: SocketAddr, datagram: Bytes) {
		let mut inbound = self.inbound.lock();
		if inbound.queue.len() >= inbound.capacity {
			tracing::warn!(
				target: "h3_listener",
				?peer,
				"virtual udp socket inbound queue full; dropping datagram",
			);
			return;
		}
		inbound.queue.push_back((peer, datagram));
		if let Some(w) = inbound.waker.take() {
			w.wake();
		}
	}

	/// Wiring point for multi-CID migration support. quinn issues new
	/// server-side CIDs as connections progress; a future PR that
	/// surfaces `quinn-proto::ConnectionEvent::NewIdentifiers` calls
	/// this to keep the listener-level dispatch table in sync.
	// TODO(s3-01-followup): wire this up against quinn 0.11+'s
	// internal new-CID event stream when the public API exposes it,
	// or drop down to quinn-proto. Until then the per-listener
	// fan-in model means dispatch is correct without external
	// registration; this method is a placeholder for the future
	// design split documented in notes/s3-01-question-virtual-socket-model.md.
	#[allow(dead_code, reason = "wiring point for multi-CID migration support")]
	pub fn register_extra_cid(self: &Arc<Self>, _cid: quinn_proto::ConnectionId) {
		// no-op until per-connection dispatch returns
	}
}

impl AsyncUdpSocket for VirtualUdpSocket {
	fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
		Box::pin(VirtualUdpPoller { socket: self })
	}

	fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
		self.physical.try_send_to(transmit.contents, transmit.destination).map(|_n| ())
	}

	fn poll_recv(
		&self,
		cx: &mut Context<'_>,
		bufs: &mut [std::io::IoSliceMut<'_>],
		meta: &mut [RecvMeta],
	) -> Poll<io::Result<usize>> {
		let mut inbound = self.inbound.lock();
		if inbound.queue.is_empty() {
			inbound.waker = Some(cx.waker().clone());
			return Poll::Pending;
		}
		let cap = bufs.len().min(meta.len()).min(inbound.queue.len());
		let local = self.physical.local_addr().unwrap_or_else(|_| {
			SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
		});
		for i in 0..cap {
			let (peer, dg) = inbound.queue.pop_front().expect("len checked");
			let n = dg.len().min(bufs[i].len());
			bufs[i][..n].copy_from_slice(&dg[..n]);
			meta[i] = RecvMeta { addr: peer, len: n, stride: n, ecn: None, dst_ip: Some(local.ip()) };
		}
		Poll::Ready(Ok(cap))
	}

	fn local_addr(&self) -> io::Result<SocketAddr> {
		self.physical.local_addr()
	}
}

/// Poller for [`VirtualUdpSocket`]. quinn calls this to register a
/// waker for "socket writable"; we proxy it to tokio's
/// `UdpSocket::poll_send_ready` since the physical socket is what we
/// actually try-send through.
struct VirtualUdpPoller {
	socket: Arc<VirtualUdpSocket>,
}

impl fmt::Debug for VirtualUdpPoller {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("VirtualUdpPoller").finish()
	}
}

impl UdpPoller for VirtualUdpPoller {
	fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.socket.physical.poll_send_ready(cx)
	}
}

/// Build the per-listener `quinn::ServerConfig` for ALPN `h3`. Reuses
/// the daemon's `Arc<rustls::ServerConfig>` (whose cert resolver is the
/// shared `VaneCertResolver`); only the ALPN list is overridden to
/// `[b"h3"]` per RFC 9114, and `enable_zero_rtt` is left at its rustls
/// default (false).
///
/// # Errors
///
/// Surfaces any `quinn::crypto::rustls` build error as a string.
pub fn build_quic_server_config(
	rustls_cfg: &Arc<rustls::ServerConfig>,
) -> Result<quinn::ServerConfig, String> {
	// Clone the rustls config and override ALPN to h3 only — H3 ALPN
	// is `h3` (RFC 9114). The original rustls config (used by the TCP
	// listener) keeps its h2/http1.1 ALPN unchanged via Arc-share.
	let inner: rustls::ServerConfig = (**rustls_cfg).clone();
	let mut h3_rustls = inner;
	h3_rustls.alpn_protocols = vec![b"h3".to_vec()];
	// TODO(s3-01-followup): TLS 1.3 0-RTT for H3 is deferred — leave
	// `enable_zero_rtt` / `max_early_data_size` at rustls defaults.
	let h3_rustls = Arc::new(h3_rustls);

	let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(h3_rustls)
		.map_err(|e| format!("quic server config: {e}"))?;
	Ok(quinn::ServerConfig::with_crypto(Arc::new(crypto)))
}
