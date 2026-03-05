/* src/layers/l4p/quic/virtual_socket.rs */

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};

/// Packet metadata from Vane passed to Quinn
/// Using Bytes for cheaper cloning if broadcast/multicast logic is added later
#[derive(Debug)]
pub struct VirtualPacket {
	pub data: bytes::Bytes,
	pub src_addr: SocketAddr,
	pub dst_addr: SocketAddr,
}

/// Virtual UDP Socket that bridges Vane's L4 Dispatcher and Quinn's Endpoint
#[derive(Debug)]
pub struct VirtualUdpSocket {
	/// Use std::sync::Mutex for poll-context safety.
	/// The critical section is extremely short (non-blocking poll).
	rx: Mutex<mpsc::Receiver<VirtualPacket>>,

	/// Physical socket for sending responses
	physical_socket: Arc<UdpSocket>,

	/// Fake local address
	local_addr: SocketAddr,
}

impl VirtualUdpSocket {
	pub fn new(
		rx: mpsc::Receiver<VirtualPacket>,
		physical_socket: Arc<UdpSocket>,
		local_addr: SocketAddr,
	) -> Self {
		Self {
			rx: Mutex::new(rx),
			physical_socket,
			local_addr,
		}
	}
}

impl AsyncUdpSocket for VirtualUdpSocket {
	fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
		// Poller mainly handles sending readiness for the OS socket
		Box::pin(VirtualPoller {
			socket: self.physical_socket.clone(),
		})
	}

	fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
		// Direct non-blocking send via physical socket
		self
			.physical_socket
			.try_send_to(transmit.contents, transmit.destination)?;
		Ok(())
	}

	fn poll_recv(
		&self,
		cx: &mut Context<'_>,
		bufs: &mut [io::IoSliceMut<'_>],
		meta: &mut [RecvMeta],
	) -> Poll<io::Result<usize>> {
		// Lock the receiver. In a single-threaded task poll, this is uncontended.
		let mut rx = self.rx.lock().unwrap();

		let mut count = 0;
		let max_packets = bufs.len().min(meta.len());

		// Batch processing loop
		while count < max_packets {
			match rx.poll_recv(cx) {
				Poll::Ready(Some(packet)) => {
					let buf = &mut bufs[count];

					// Truncate if packet is larger than buffer (UDP semantic)
					let len = packet.data.len().min(buf.len());
					buf[..len].copy_from_slice(&packet.data[..len]);

					meta[count] = RecvMeta {
						addr: packet.src_addr,
						len,
						stride: len,
						ecn: None,
						dst_ip: Some(packet.dst_addr.ip()),
					};

					count += 1;
				}
				Poll::Ready(None) => {
					// Channel closed. If we read some packets, return them.
					// Otherwise, signal broken pipe.
					if count > 0 {
						break;
					}
					return Poll::Ready(Err(io::Error::new(
						io::ErrorKind::BrokenPipe,
						"Virtual socket channel closed",
					)));
				}
				Poll::Pending => {
					// No more packets available right now.
					break;
				}
			}
		}

		if count > 0 {
			Poll::Ready(Ok(count))
		} else {
			// We read 0 packets and the channel returned Pending.
			// The channel has already registered the waker from cx.
			Poll::Pending
		}
	}

	fn local_addr(&self) -> io::Result<SocketAddr> {
		Ok(self.local_addr)
	}

	fn may_fragment(&self) -> bool {
		false
	}
}

#[derive(Debug)]
struct VirtualPoller {
	socket: Arc<UdpSocket>,
}

impl UdpPoller for VirtualPoller {
	fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.socket.poll_send_ready(cx)
	}
}
