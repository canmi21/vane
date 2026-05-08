//! Adapter that exposes a [`virtual_socket::VirtualUdpSocket`] as a
//! [`quinn::AsyncUdpSocket`], so a [`quinn::Endpoint`] can run on a
//! UDP socket that is shared with other consumers.
//!
//! Typical setup:
//!
//! 1. A "router" task owns the physical [`tokio::net::UdpSocket`] and
//!    `recv_from`s it.
//! 2. Each consumer (a `quinn::Endpoint`, an L4 forwarder, a DNS
//!    parser, ...) gets its own [`virtual_socket::VirtualUdpSocket`]
//!    backed by the same physical socket.
//! 3. For inbound traffic, the router applies its own demultiplex
//!    rule and pushes each datagram onto the matching virtual
//!    socket's queue with [`virtual_socket::VirtualUdpSocket::enqueue_inbound`].
//! 4. The QUIC consumer wraps its virtual socket in
//!    [`SharedSocket::new`] and hands the wrapper to
//!    [`quinn::Endpoint::new_with_abstract_socket`]. quinn then sees
//!    the virtual socket as if it were exclusive — it can demux QUIC
//!    Connection IDs internally without knowing about the shared
//!    physical layer.
//!
//! For outbound, [`SharedSocket`] forwards every transmit through
//! `virtual_socket`'s `try_send_to`, which in turn writes through to
//! the physical socket. quinn never owns the physical socket, so
//! other consumers can keep writing through it concurrently.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use virtual_socket::VirtualUdpSocket;

/// Adapter that satisfies [`quinn::AsyncUdpSocket`] for a
/// [`virtual_socket::VirtualUdpSocket`].
///
/// Construct via [`SharedSocket::new`], then hand the resulting
/// `Arc<SharedSocket>` to
/// [`quinn::Endpoint::new_with_abstract_socket`].
pub struct SharedSocket {
	inner: Arc<VirtualUdpSocket>,
}

impl SharedSocket {
	/// Wrap a [`virtual_socket::VirtualUdpSocket`].
	#[must_use]
	pub fn new(inner: Arc<VirtualUdpSocket>) -> Arc<Self> {
		Arc::new(Self { inner })
	}

	/// Borrow the underlying virtual socket — useful if the caller
	/// needs to reach the inbound-enqueue side from the same value.
	#[must_use]
	pub fn virtual_socket(&self) -> &Arc<VirtualUdpSocket> {
		&self.inner
	}
}

impl fmt::Debug for SharedSocket {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("SharedSocket").field("inner", &self.inner).finish()
	}
}

impl AsyncUdpSocket for SharedSocket {
	fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
		Box::pin(SharedSocketPoller { socket: self })
	}

	fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
		self.inner.try_send_to(transmit.contents, transmit.destination).map(|_n| ())
	}

	fn poll_recv(
		&self,
		cx: &mut Context<'_>,
		bufs: &mut [io::IoSliceMut<'_>],
		meta: &mut [RecvMeta],
	) -> Poll<io::Result<usize>> {
		let max = bufs.len().min(meta.len());
		if max == 0 {
			return Poll::Ready(Ok(0));
		}

		// First slot: must register a waker if no datagram is queued.
		// `Ready(None)` (closed + drained) surfaces as ConnectionAborted
		// so quinn's accept loop tears the endpoint down cleanly.
		let first = match self.inner.poll_dequeue(cx) {
			Poll::Ready(Some(d)) => d,
			Poll::Ready(None) => {
				return Poll::Ready(Err(io::Error::new(
					io::ErrorKind::ConnectionAborted,
					"virtual socket closed",
				)));
			}
			Poll::Pending => return Poll::Pending,
		};

		let local = self.inner.local_addr().unwrap_or_else(|_| SocketAddr::from(([0u8, 0, 0, 0], 0)));

		fill_slot(0, first, bufs, meta, local);
		let mut count = 1;

		// Drain remaining buf slots non-blockingly so a burst of
		// datagrams completes in one wake-up.
		while count < max {
			match self.inner.try_dequeue() {
				Some(d) => {
					fill_slot(count, d, bufs, meta, local);
					count += 1;
				}
				None => break,
			}
		}
		Poll::Ready(Ok(count))
	}

	fn local_addr(&self) -> io::Result<SocketAddr> {
		self.inner.local_addr()
	}
}

fn fill_slot(
	idx: usize,
	datagram: (SocketAddr, bytes::Bytes),
	bufs: &mut [io::IoSliceMut<'_>],
	meta: &mut [RecvMeta],
	local: SocketAddr,
) {
	let (peer, payload) = datagram;
	let n = payload.len().min(bufs[idx].len());
	bufs[idx][..n].copy_from_slice(&payload[..n]);
	meta[idx] = RecvMeta { addr: peer, len: n, stride: n, ecn: None, dst_ip: Some(local.ip()) };
}

/// Poller for [`SharedSocket`]. quinn calls this to register a waker
/// for "socket writable"; we forward to the underlying physical
/// socket via [`virtual_socket::VirtualUdpSocket::poll_send_ready`].
struct SharedSocketPoller {
	socket: Arc<SharedSocket>,
}

impl fmt::Debug for SharedSocketPoller {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("SharedSocketPoller").finish()
	}
}

impl UdpPoller for SharedSocketPoller {
	fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.socket.inner.poll_send_ready(cx)
	}
}

#[cfg(test)]
mod tests {
	use std::future::poll_fn;
	use std::net::Ipv4Addr;

	use bytes::Bytes;
	use quinn::AsyncUdpSocket;
	use tokio::net::UdpSocket;

	use super::*;

	async fn bound() -> Arc<UdpSocket> {
		Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("bind"))
	}

	#[tokio::test]
	async fn local_addr_passes_through() {
		let phys = bound().await;
		let want = phys.local_addr().expect("local addr");
		let virt = VirtualUdpSocket::new(phys);
		let shared = SharedSocket::new(virt);
		assert_eq!(<SharedSocket as AsyncUdpSocket>::local_addr(&shared).unwrap(), want);
	}

	#[tokio::test]
	async fn poll_recv_pending_when_queue_empty() {
		let phys = bound().await;
		let virt = VirtualUdpSocket::new(phys);
		let shared = SharedSocket::new(virt);

		// Single-poll: no datagrams queued => Pending.
		let mut storage = [0u8; 64];
		let mut bufs = [io::IoSliceMut::new(&mut storage)];
		let mut metas = [RecvMeta::default()];
		let r = std::future::poll_fn(|cx| {
			match <SharedSocket as AsyncUdpSocket>::poll_recv(&shared, cx, &mut bufs, &mut metas) {
				Poll::Pending => Poll::Ready(()),
				ready @ Poll::Ready(_) => panic!("expected Pending, got {ready:?}"),
			}
		})
		.await;
		let () = r;
	}

	#[tokio::test]
	async fn poll_recv_returns_queued_datagram() {
		let phys = bound().await;
		let virt = VirtualUdpSocket::new(phys);
		let peer: SocketAddr = "192.0.2.10:443".parse().unwrap();
		virt.enqueue_inbound(peer, Bytes::from_static(b"INIT"));
		let shared = SharedSocket::new(virt);

		let mut buf = [0u8; 64];
		let mut bufs = [io::IoSliceMut::new(&mut buf)];
		let mut metas = [RecvMeta::default()];
		let n =
			poll_fn(|cx| <SharedSocket as AsyncUdpSocket>::poll_recv(&shared, cx, &mut bufs, &mut metas))
				.await
				.expect("poll_recv ok");
		assert_eq!(n, 1);
		assert_eq!(metas[0].addr, peer);
		assert_eq!(metas[0].len, 4);
		assert_eq!(&buf[..4], b"INIT");
	}

	#[tokio::test]
	async fn poll_recv_drains_burst_into_multi_slot_call() {
		let phys = bound().await;
		let virt = VirtualUdpSocket::new(phys);
		let peer1: SocketAddr = "192.0.2.11:443".parse().unwrap();
		let peer2: SocketAddr = "192.0.2.12:443".parse().unwrap();
		virt.enqueue_inbound(peer1, Bytes::from_static(b"A"));
		virt.enqueue_inbound(peer2, Bytes::from_static(b"BB"));
		let shared = SharedSocket::new(virt);

		let mut b1 = [0u8; 16];
		let mut b2 = [0u8; 16];
		let mut bufs = [io::IoSliceMut::new(&mut b1), io::IoSliceMut::new(&mut b2)];
		let mut metas = [RecvMeta::default(), RecvMeta::default()];
		let n =
			poll_fn(|cx| <SharedSocket as AsyncUdpSocket>::poll_recv(&shared, cx, &mut bufs, &mut metas))
				.await
				.expect("poll_recv ok");
		assert_eq!(n, 2);
		assert_eq!(metas[0].addr, peer1);
		assert_eq!(metas[1].addr, peer2);
		assert_eq!(&b1[..1], b"A");
		assert_eq!(&b2[..2], b"BB");
	}

	#[tokio::test]
	async fn poll_recv_surfaces_close_as_connection_aborted() {
		let phys = bound().await;
		let virt = VirtualUdpSocket::new(phys);
		virt.close();
		let shared = SharedSocket::new(virt);

		let mut buf = [0u8; 16];
		let mut bufs = [io::IoSliceMut::new(&mut buf)];
		let mut metas = [RecvMeta::default()];
		let r =
			poll_fn(|cx| <SharedSocket as AsyncUdpSocket>::poll_recv(&shared, cx, &mut bufs, &mut metas))
				.await;
		let err = r.expect_err("close => err");
		assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
	}

	#[tokio::test]
	async fn try_send_proxies_to_physical() {
		let phys_src = bound().await;
		let phys_dst = bound().await;
		let dst_addr = phys_dst.local_addr().unwrap();
		let virt = VirtualUdpSocket::new(phys_src);
		let shared = SharedSocket::new(virt);

		// Wait for OS-side writability before try_send.
		poll_fn(|cx| shared.virtual_socket().poll_send_ready(cx)).await.expect("ready");
		<SharedSocket as AsyncUdpSocket>::try_send(
			&shared,
			&Transmit {
				destination: dst_addr,
				ecn: None,
				contents: b"PING",
				segment_size: None,
				src_ip: None,
			},
		)
		.expect("try_send");
		let mut got = [0u8; 16];
		let (n, _) = phys_dst.recv_from(&mut got).await.expect("recv");
		assert_eq!(&got[..n], b"PING");
	}
}
