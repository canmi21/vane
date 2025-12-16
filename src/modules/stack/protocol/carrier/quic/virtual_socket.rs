/* src/modules/stack/protocol/carrier/quic/virtual_socket.rs */

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// A Virtual UDP Socket that bridges Vane's L4 Dispatcher and Quinn's Endpoint.
///
/// - **Reading:** Consumes packets from an internal MPSC channel (fed by Vane).
/// - **Writing:** Sends packets directly via the shared physical UdpSocket (owned by Vane).
#[derive(Debug)]
pub struct VirtualUdpSocket {
	/// Channel to receive packets from Vane's dispatcher (The "Feed")
	rx: mpsc::Receiver<(Vec<u8>, SocketAddr)>,

	/// Reference to the physical socket for sending responses
	physical_socket: Arc<UdpSocket>,
}

impl VirtualUdpSocket {
	pub fn new(rx: mpsc::Receiver<(Vec<u8>, SocketAddr)>, physical_socket: Arc<UdpSocket>) -> Self {
		Self {
			rx,
			physical_socket,
		}
	}
}

// Implement the Async IO traits required by Quinn (via Tokio Runtime compatibility).
// Quinn typically expects a type that behaves like tokio::net::UdpSocket.
// Since we cannot implement `tokio::net::UdpSocket` (it's a struct),
// we rely on Quinn's ability to use any type implementing `quinn::AsyncUdpSocket`
// IF we were using the generic runtime.
//
// However, fitting this into standard Quinn Endpoint usually requires the `quinn_udp` traits.
// For the sake of this handover, we define the IO logic methods which the Muxer will use
// to drive the connection.

impl VirtualUdpSocket {
	/// Simulates receiving a packet from the network (actually from Vane's channel).
	pub fn poll_recv(
		&mut self,
		cx: &mut Context<'_>,
		buf: &mut [u8],
	) -> Poll<io::Result<(usize, SocketAddr)>> {
		match self.rx.poll_recv(cx) {
			Poll::Ready(Some((data, addr))) => {
				let len = std::cmp::min(buf.len(), data.len());
				buf[..len].copy_from_slice(&data[..len]);
				Poll::Ready(Ok((len, addr)))
			}
			Poll::Ready(None) => Poll::Ready(Err(io::Error::new(
				io::ErrorKind::BrokenPipe,
				"Virtual socket channel closed",
			))),
			Poll::Pending => Poll::Pending,
		}
	}

	/// Simulates sending a packet to the network (actually uses the physical socket).
	pub fn poll_send(
		&self,
		cx: &mut Context<'_>,
		buf: &[u8],
		target: SocketAddr,
	) -> Poll<io::Result<usize>> {
		// We use the physical socket to send.
		// Note: poll_send_to is basically async send_to.
		// Since Arc<UdpSocket> handles concurrency, this is safe.
		self.physical_socket.poll_send_to(cx, buf, target)
	}
}

// Adapter for Quinn's Runtime (Conceptual).
// Since wiring up a full custom Runtime for Quinn is verbose,
// we will instantiate the Endpoint using a standard socket bound to localhost
// for the internal state machine, but we override the IO loop in the Muxer.
// OR, more cleanly:
// We provide the Muxer that manages `quinn::Endpoint` via a hack:
// We don't use this struct *inside* Quinn (since Quinn creates its own socket),
// we use this struct to *proxy* traffic to a Quinn instance running on a loopback port.
//
// WAIT: The User requested "Internal create a virtual udp socket... held by quinn".
// This implies we DO want to inject it.
// The only way to inject a socket into `quinn` is via `Endpoint::new_with_abstract_socket`.
// This requires `quinn` features `runtime-tokio` and specific trait bounds.
