//! Virtual UDP sockets that share a single physical [`tokio::net::UdpSocket`].
//!
//! Pattern: a single physical UDP socket is owned by a top-level
//! "router" task that reads from it. The router demultiplexes each
//! inbound datagram to one of several [`VirtualUdpSocket`]s — by peer
//! address, by some identifier inside the payload (e.g. a QUIC
//! Connection ID), by listener kind, or any other rule the router
//! chooses. Each virtual socket has its own bounded inbound queue;
//! consumers drain that queue via [`VirtualUdpSocket::poll_dequeue`]
//! or [`VirtualUdpSocket::try_dequeue`].
//!
//! Outbound is mux: every virtual socket forwards
//! [`VirtualUdpSocket::try_send_to`] / [`VirtualUdpSocket::poll_send_ready`]
//! to the shared physical socket, so multiple consumers can write
//! through the same OS endpoint without contention beyond what the OS
//! itself imposes.
//!
//! This crate is transport-policy free: it does not parse datagrams,
//! does not own a routing table, and does not implement any
//! application protocol. Pair it with whatever demultiplex strategy
//! the calling system needs (e.g. peer-address fan-in, QUIC CID
//! demux, DNS query-ID dispatch).
//!
//! For an adapter that exposes [`VirtualUdpSocket`] as a
//! [`quinn::AsyncUdpSocket`](https://docs.rs/quinn), see the
//! `quinn-shared-socket` crate.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::net::UdpSocket;

/// Default inbound queue capacity. Matches a single high-throughput
/// QUIC endpoint's burst window without letting a single misbehaving
/// session starve siblings sharing the physical socket.
pub const DEFAULT_INBOUND_CAPACITY: usize = 256;

/// One virtual UDP socket sharing the physical socket given at
/// construction time. Inbound datagrams arrive only via
/// [`VirtualUdpSocket::enqueue_inbound`] (called by the router that
/// owns the physical socket); outbound datagrams pass through to the
/// physical socket unchanged.
///
/// Cloning the `Arc<VirtualUdpSocket>` shares state — both clones see
/// the same inbound queue, the same physical socket, and the same
/// closed flag. This is intended: typical use installs one clone in
/// the router's dispatch table and hands another to the consumer
/// task.
pub struct VirtualUdpSocket {
	physical: Arc<UdpSocket>,
	inbound: Mutex<Inbound>,
	closed: AtomicBool,
}

struct Inbound {
	queue: VecDeque<(SocketAddr, Bytes)>,
	waker: Option<Waker>,
	capacity: usize,
}

impl std::fmt::Debug for VirtualUdpSocket {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("VirtualUdpSocket")
			.field("closed", &self.closed.load(Ordering::Relaxed))
			.finish_non_exhaustive()
	}
}

impl VirtualUdpSocket {
	/// Build a virtual socket against `physical` with
	/// [`DEFAULT_INBOUND_CAPACITY`].
	#[must_use]
	pub fn new(physical: Arc<UdpSocket>) -> Arc<Self> {
		Self::new_with_capacity(physical, DEFAULT_INBOUND_CAPACITY)
	}

	/// Build a virtual socket against `physical` with a custom
	/// inbound queue capacity. `capacity` of zero is allowed but
	/// drops every enqueue.
	#[must_use]
	pub fn new_with_capacity(physical: Arc<UdpSocket>, capacity: usize) -> Arc<Self> {
		Arc::new(Self {
			physical,
			inbound: Mutex::new(Inbound { queue: VecDeque::new(), waker: None, capacity }),
			closed: AtomicBool::new(false),
		})
	}

	/// Local address of the underlying physical socket.
	///
	/// # Errors
	///
	/// Surfaces the OS error from [`tokio::net::UdpSocket::local_addr`].
	pub fn local_addr(&self) -> io::Result<SocketAddr> {
		self.physical.local_addr()
	}

	/// Push one datagram onto this virtual socket's inbound queue.
	/// Called by the router that owns the physical socket. Drops the
	/// datagram silently when the queue is full or the socket has
	/// been closed — UDP is lossy by design and stalling the router
	/// would block every other virtual socket sharing the physical
	/// endpoint. A `tracing::warn!` records each drop.
	pub fn enqueue_inbound(&self, peer: SocketAddr, datagram: Bytes) {
		if self.closed.load(Ordering::Relaxed) {
			tracing::warn!(?peer, "virtual udp socket closed; dropping inbound datagram");
			return;
		}
		let mut inbound = self.inbound.lock();
		if inbound.queue.len() >= inbound.capacity {
			tracing::warn!(?peer, "virtual udp socket inbound queue full; dropping datagram");
			return;
		}
		inbound.queue.push_back((peer, datagram));
		if let Some(w) = inbound.waker.take() {
			w.wake();
		}
	}

	/// Pop the head of the inbound queue without blocking. Returns
	/// `None` when the queue is empty (regardless of closed state).
	pub fn try_dequeue(&self) -> Option<(SocketAddr, Bytes)> {
		self.inbound.lock().queue.pop_front()
	}

	/// Poll for the next inbound datagram.
	///
	/// - `Poll::Ready(Some((peer, datagram)))` when a datagram is
	///   available.
	/// - `Poll::Ready(None)` when [`Self::close`] has been called and
	///   the queue has been fully drained — the caller can treat
	///   this as a clean end-of-stream.
	/// - `Poll::Pending` otherwise; the waker from `cx` is registered
	///   and woken on the next [`Self::enqueue_inbound`] /
	///   [`Self::close`].
	pub fn poll_dequeue(&self, cx: &mut Context<'_>) -> Poll<Option<(SocketAddr, Bytes)>> {
		let mut inbound = self.inbound.lock();
		if let Some(item) = inbound.queue.pop_front() {
			return Poll::Ready(Some(item));
		}
		if self.closed.load(Ordering::Relaxed) {
			return Poll::Ready(None);
		}
		inbound.waker = Some(cx.waker().clone());
		Poll::Pending
	}

	/// Forward an outbound datagram to the physical socket without
	/// blocking. Surfaces `WouldBlock` to the caller — wait via
	/// [`Self::poll_send_ready`] before retrying.
	///
	/// # Errors
	///
	/// Surfaces the OS error from
	/// [`tokio::net::UdpSocket::try_send_to`].
	pub fn try_send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
		self.physical.try_send_to(buf, target)
	}

	/// Wait until the physical socket is writable. Proxies to
	/// [`tokio::net::UdpSocket::poll_send_ready`].
	///
	/// # Errors
	///
	/// Surfaces the OS error from `poll_send_ready`.
	pub fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.physical.poll_send_ready(cx)
	}

	/// Borrow the underlying physical socket. Useful when callers
	/// need to invoke socket-level methods (e.g. setting buffer
	/// sizes) that virtual-socket does not surface.
	#[must_use]
	pub fn physical(&self) -> &Arc<UdpSocket> {
		&self.physical
	}

	/// Mark this virtual socket closed. New
	/// [`Self::enqueue_inbound`] calls are dropped; existing queued
	/// datagrams remain drainable. Once the queue is empty,
	/// [`Self::poll_dequeue`] returns `Poll::Ready(None)` so consumer
	/// tasks can exit cleanly. Idempotent.
	pub fn close(&self) {
		let already = self.closed.swap(true, Ordering::Relaxed);
		if !already {
			// Wake the consumer so it observes the close on the next
			// poll, even if no datagrams arrive after this point.
			if let Some(w) = self.inbound.lock().waker.take() {
				w.wake();
			}
		}
	}

	/// Whether [`Self::close`] has been called.
	#[must_use]
	pub fn is_closed(&self) -> bool {
		self.closed.load(Ordering::Relaxed)
	}

	/// Inbound queue length. Useful for metrics / diagnostics.
	#[must_use]
	pub fn inbound_len(&self) -> usize {
		self.inbound.lock().queue.len()
	}
}

#[cfg(test)]
mod tests {
	use std::future::poll_fn;
	use std::net::Ipv4Addr;

	use bytes::Bytes;
	use tokio::net::UdpSocket;

	use super::*;

	async fn bound() -> (Arc<UdpSocket>, SocketAddr) {
		let s = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("bind");
		let a = s.local_addr().expect("local_addr");
		(Arc::new(s), a)
	}

	#[tokio::test]
	async fn try_dequeue_returns_none_when_empty() {
		let (phys, _) = bound().await;
		let v = VirtualUdpSocket::new(phys);
		assert!(v.try_dequeue().is_none());
	}

	#[tokio::test]
	async fn enqueue_then_dequeue_roundtrip() {
		let (phys, _) = bound().await;
		let v = VirtualUdpSocket::new(phys);
		let peer: SocketAddr = "192.0.2.1:443".parse().unwrap();
		v.enqueue_inbound(peer, Bytes::from_static(b"hello"));
		v.enqueue_inbound(peer, Bytes::from_static(b"world"));
		let (p1, d1) = v.try_dequeue().unwrap();
		assert_eq!(p1, peer);
		assert_eq!(&*d1, b"hello");
		let (_, d2) = v.try_dequeue().unwrap();
		assert_eq!(&*d2, b"world");
		assert!(v.try_dequeue().is_none());
	}

	#[tokio::test]
	async fn poll_dequeue_pending_then_woken_on_enqueue() {
		let (phys, _) = bound().await;
		let v = VirtualUdpSocket::new(phys);
		let peer: SocketAddr = "192.0.2.2:443".parse().unwrap();
		let v_for_task = Arc::clone(&v);
		let waker_task = tokio::spawn(async move { poll_fn(|cx| v_for_task.poll_dequeue(cx)).await });
		// Yield so the task registers its waker.
		tokio::task::yield_now().await;
		v.enqueue_inbound(peer, Bytes::from_static(b"X"));
		let (got_peer, got_data) = waker_task.await.unwrap().expect("dequeue ok");
		assert_eq!(got_peer, peer);
		assert_eq!(&*got_data, b"X");
	}

	#[tokio::test]
	async fn full_queue_drops_overflow() {
		let (phys, _) = bound().await;
		let v = VirtualUdpSocket::new_with_capacity(phys, 2);
		let peer: SocketAddr = "192.0.2.3:443".parse().unwrap();
		v.enqueue_inbound(peer, Bytes::from_static(&[1]));
		v.enqueue_inbound(peer, Bytes::from_static(&[2]));
		// This one is dropped (capacity is 2).
		v.enqueue_inbound(peer, Bytes::from_static(&[3]));
		assert_eq!(v.inbound_len(), 2);
		assert_eq!(&*v.try_dequeue().unwrap().1, &[1]);
		assert_eq!(&*v.try_dequeue().unwrap().1, &[2]);
		assert!(v.try_dequeue().is_none());
	}

	#[tokio::test]
	async fn close_drops_subsequent_enqueues_and_yields_none_after_drain() {
		let (phys, _) = bound().await;
		let v = VirtualUdpSocket::new(phys);
		let peer: SocketAddr = "192.0.2.4:443".parse().unwrap();
		v.enqueue_inbound(peer, Bytes::from_static(b"A"));
		v.close();
		// Existing items still drain.
		assert_eq!(&*v.try_dequeue().unwrap().1, b"A");
		// New enqueues are dropped.
		v.enqueue_inbound(peer, Bytes::from_static(b"B"));
		assert!(v.try_dequeue().is_none());
		// poll_dequeue returns Ready(None) after drain + close.
		let r = poll_fn(|cx| v.poll_dequeue(cx)).await;
		assert!(r.is_none());
	}

	#[tokio::test]
	async fn try_send_to_proxies_physical() {
		let (phys_a, addr_a) = bound().await;
		let (phys_b, addr_b) = bound().await;
		let v = VirtualUdpSocket::new(phys_a);
		// Outbound from v reaches phys_b.
		// poll_send_ready on the physical socket so we don't race the OS buffer state.
		poll_fn(|cx| v.poll_send_ready(cx)).await.expect("send_ready");
		let n = v.try_send_to(b"PING", addr_b).expect("send");
		assert_eq!(n, 4);
		let mut buf = [0u8; 16];
		let (got, from) = phys_b.recv_from(&mut buf).await.expect("recv");
		assert_eq!(&buf[..got], b"PING");
		assert_eq!(from, addr_a);
	}

	#[tokio::test]
	async fn local_addr_matches_physical() {
		let (phys, addr) = bound().await;
		let v = VirtualUdpSocket::new(phys);
		assert_eq!(v.local_addr().unwrap(), addr);
	}
}
