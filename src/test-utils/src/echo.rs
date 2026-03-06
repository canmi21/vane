use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A single-connection TCP echo server for testing.
///
/// Accepts one connection, reads one chunk of data, writes it back,
/// then shuts down.
pub struct EchoServer {
	addr: SocketAddr,
	handle: JoinHandle<()>,
}

impl EchoServer {
	pub async fn start() -> Self {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let handle = tokio::spawn(async move {
			let (mut stream, _) = listener.accept().await.unwrap();
			let mut buf = vec![0u8; 8192];
			let n = stream.read(&mut buf).await.unwrap();
			if n > 0 {
				stream.write_all(&buf[..n]).await.unwrap();
			}
			stream.shutdown().await.unwrap();
		});
		Self { addr, handle }
	}

	pub const fn addr(&self) -> SocketAddr {
		self.addr
	}

	pub async fn join(self) {
		let _ = self.handle.await;
	}
}
