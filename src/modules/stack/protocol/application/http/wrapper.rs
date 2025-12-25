/* src/modules/stack/protocol/application/http/wrapper.rs */

use crate::common::requirements::Error;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::{Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::upgrade::OnUpgrade;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

/// A unified Body enum that bridges Hyper (H1/H2), H3 (Quinn), and Buffered data.
pub enum VaneBody {
	/// Native Hyper Body (HTTP/1.1, HTTP/2)
	Hyper(Incoming),

	/// H3 Stream Wrapper
	H3(BoxBody<Bytes, Error>),

	/// Generic Stream Wrapper (Boxed, for plugins like CGI/FastCGI)
	Generic(BoxBody<Bytes, Error>),

	/// Buffered Memory (Lazy Buffer or Generated Content)
	Buffered(Full<Bytes>),

	/// Special State: Switching Protocols (WebSocket / Upgrade)
	/// Holds the upstream upgrade handle.
	SwitchingProtocols(OnUpgrade),

	/// A bridge that executes a callback when polled.
	/// Used to spawn the tunnel task at the exact right moment.
	/// Added + Sync to the trait object to satisfy BodyExt::boxed() bounds.
	UpgradeBridge {
		tunnel_task: Option<Pin<Box<dyn Future<Output = ()> + Send + Sync>>>,
	},

	/// Empty Body
	Empty,
}

impl std::fmt::Debug for VaneBody {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Hyper(_) => write!(f, "Hyper"),
			Self::H3(_) => write!(f, "H3"),
			Self::Generic(_) => write!(f, "Generic"),
			Self::Buffered(_) => write!(f, "Buffered"),
			Self::SwitchingProtocols(_) => write!(f, "SwitchingProtocols(Pending)"),
			Self::UpgradeBridge { .. } => write!(f, "UpgradeBridge(Ready)"),
			Self::Empty => write!(f, "Empty"),
		}
	}
}

impl Default for VaneBody {
	fn default() -> Self {
		Self::Empty
	}
}

impl Body for VaneBody {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		match &mut *self {
			Self::Hyper(body) => match Pin::new(body).poll_frame(cx) {
				Poll::Ready(Some(Ok(frame))) => {
					let frame = frame.map_data(|d| d);
					Poll::Ready(Some(Ok(frame)))
				}
				Poll::Ready(Some(Err(e))) => {
					Poll::Ready(Some(Err(Error::System(format!("Hyper Body Error: {}", e)))))
				}
				Poll::Ready(None) => Poll::Ready(None),
				Poll::Pending => Poll::Pending,
			},
			Self::H3(body) => Pin::new(body).poll_frame(cx),
			Self::Generic(body) => Pin::new(body).poll_frame(cx),
			Self::Buffered(body) => match Pin::new(body).poll_frame(cx) {
				Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
				Poll::Ready(Some(Err(e))) => match e {},
				Poll::Ready(None) => Poll::Ready(None),
				Poll::Pending => Poll::Pending,
			},
			Self::SwitchingProtocols(_) => Poll::Ready(None),
			Self::UpgradeBridge { tunnel_task } => {
				if let Some(task) = tunnel_task.take() {
					tokio::spawn(task);
				}
				Poll::Ready(None)
			}
			Self::Empty => Poll::Ready(None),
		}
	}

	fn is_end_stream(&self) -> bool {
		match self {
			Self::Hyper(b) => b.is_end_stream(),
			Self::H3(b) => b.is_end_stream(),
			Self::Generic(b) => b.is_end_stream(),
			Self::Buffered(b) => b.is_end_stream(),
			Self::SwitchingProtocols(_) => true,
			Self::UpgradeBridge { .. } => true,
			Self::Empty => true,
		}
	}

	fn size_hint(&self) -> SizeHint {
		match self {
			Self::Hyper(b) => b.size_hint(),
			Self::H3(b) => b.size_hint(),
			Self::Generic(b) => b.size_hint(),
			Self::Buffered(b) => b.size_hint(),
			Self::SwitchingProtocols(_) => SizeHint::with_exact(0),
			Self::UpgradeBridge { .. } => SizeHint::with_exact(0),
			Self::Empty => SizeHint::with_exact(0),
		}
	}
}

pub struct H3BodyAdapter {
	rx: mpsc::Receiver<Result<Bytes, Error>>,
}

impl H3BodyAdapter {
	pub fn new(rx: mpsc::Receiver<Result<Bytes, Error>>) -> Self {
		Self { rx }
	}
}

impl Body for H3BodyAdapter {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		match self.rx.poll_recv(cx) {
			Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(Frame::data(bytes)))),
			Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}
