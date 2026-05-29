//! Echo HTTP / TCP servers with auto-teardown on `Drop` (`EchoServer`).
//! UDP echo + TLS fixtures land with their respective protocol features.
//!
//! See [`spec/conventions.md` § _Testing_](../../../spec/conventions.md#testing).

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// A mock HTTP/1.1 upstream that answers every request with a fixed
/// `200 OK` carrying a known body. Deliberately raw-TCP (no hyper, no
/// tokio) so it runs on its own OS thread, stays independent of the
/// caller's async runtime, and survives the partial-read / half-close
/// edge cases an L4-forward or reverse-proxy test pushes through it.
///
/// Bound on `127.0.0.1:0`; the OS-assigned address is exposed via
/// [`EchoServer::addr`]. The accept loop stops on `Drop` (a self-connect
/// wakes the blocking `accept`, then the loop observes the stop flag).
pub struct EchoServer {
	addr: SocketAddr,
	stop: Arc<AtomicBool>,
	handle: Option<JoinHandle<()>>,
}

impl EchoServer {
	/// Bind an ephemeral loopback port and serve `body` to every request.
	pub fn start(body: impl Into<String>) -> Self {
		let listener = TcpListener::bind("127.0.0.1:0").expect("bind echo upstream");
		let addr = listener.local_addr().expect("echo local_addr");
		let body = body.into();
		let stop = Arc::new(AtomicBool::new(false));
		let stop_for_thread = Arc::clone(&stop);
		let handle = std::thread::spawn(move || {
			for conn in listener.incoming() {
				if stop_for_thread.load(Ordering::Relaxed) {
					break;
				}
				let Ok(mut sock) = conn else { continue };
				// Drain the request head best-effort; we never parse it.
				let mut scratch = [0u8; 2048];
				let _ = sock.read(&mut scratch);
				let resp = format!(
					"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
					body.len(),
				);
				let _ = sock.write_all(resp.as_bytes());
				let _ = sock.flush();
			}
		});
		Self { addr, stop, handle: Some(handle) }
	}

	/// The OS-assigned `127.0.0.1:<port>` this upstream is listening on.
	pub fn addr(&self) -> SocketAddr {
		self.addr
	}
}

impl Drop for EchoServer {
	fn drop(&mut self) {
		self.stop.store(true, Ordering::Relaxed);
		// Wake the blocking `accept` so the loop can observe the flag.
		let _ = TcpStream::connect(self.addr);
		if let Some(h) = self.handle.take() {
			let _ = h.join();
		}
	}
}
