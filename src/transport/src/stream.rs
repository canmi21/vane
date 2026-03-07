use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;

/// Unified stream type for both plain TCP and TLS connections.
///
/// The TLS variant is boxed to avoid inflating every `ConnectionStream` to the
/// size of `TlsStream` (~1200 bytes); plain TCP stays inline (~40 bytes).
pub enum ConnectionStream {
	Tcp(TcpStream),
	Tls(Box<TlsStream<TcpStream>>),
}

impl ConnectionStream {
	/// Extract the inner `TcpStream`, consuming self.
	///
	/// Returns `None` for TLS streams (the raw TCP stream is owned by the TLS layer).
	pub fn into_tcp(self) -> Option<TcpStream> {
		match self {
			Self::Tcp(s) => Some(s),
			Self::Tls(_) => None,
		}
	}
}

impl From<TcpStream> for ConnectionStream {
	fn from(s: TcpStream) -> Self {
		Self::Tcp(s)
	}
}

impl From<TlsStream<TcpStream>> for ConnectionStream {
	fn from(s: TlsStream<TcpStream>) -> Self {
		Self::Tls(Box::new(s))
	}
}

impl AsyncRead for ConnectionStream {
	fn poll_read(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut ReadBuf<'_>,
	) -> Poll<io::Result<()>> {
		match self.get_mut() {
			Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
			Self::Tls(s) => Pin::new(s).poll_read(cx, buf),
		}
	}
}

impl AsyncWrite for ConnectionStream {
	fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
		match self.get_mut() {
			Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
			Self::Tls(s) => Pin::new(s).poll_write(cx, buf),
		}
	}

	fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		match self.get_mut() {
			Self::Tcp(s) => Pin::new(s).poll_flush(cx),
			Self::Tls(s) => Pin::new(s).poll_flush(cx),
		}
	}

	fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		match self.get_mut() {
			Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
			Self::Tls(s) => Pin::new(s).poll_shutdown(cx),
		}
	}
}
