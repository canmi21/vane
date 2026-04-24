use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;

use crate::body::{Request, Response};
use crate::conn_context::ConnContext;
use crate::error::Error;
use crate::flow_ctx::FlowCtx;
use crate::l4::L4Conn;
use crate::middleware::CloseReason;

#[async_trait]
pub trait L7Fetch: Send + Sync {
	async fn fetch(
		&self,
		req: Request,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx<'_>,
	) -> Result<L7FetchOutput, Error>;
}

#[async_trait]
pub trait L4Fetch: Send + Sync {
	async fn fetch(
		&self,
		l4: L4Conn,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx<'_>,
	) -> Result<Tunnel, Error>;
}

pub enum L7FetchOutput {
	Response(Response),
	Tunnel(Tunnel),
}

pub struct Tunnel {
	pub client: Pin<Box<dyn AsyncReadWrite + Send>>,
	pub upstream: Pin<Box<dyn AsyncReadWrite + Send>>,
	pub close_reason_tx: Option<oneshot::Sender<CloseReason>>,
}

pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> AsyncReadWrite for T {}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum FetchKind {
	HttpProxy,
	HttpSynthesize,
	WebSocketUpgrade,
	L4Forward,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum FetchPhase {
	L4,
	L7,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct FetchOutputModes {
	pub response: bool,
	pub tunnel: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SymbolicFetchRef {
	pub kind: FetchKind,
	pub args: serde_json::Value,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Terminator {
	WriteHttpResponse,
	ByteTunnel,
}

#[cfg(test)]
mod tests {
	use std::future::Future;
	use std::io;
	use std::net::SocketAddr;
	use std::task::{Context, Poll};
	use std::time::Instant;

	use parking_lot::Mutex;
	use serde_json::json;
	use tokio::io::ReadBuf;
	use tokio_util::sync::CancellationToken;

	use super::*;
	use crate::body::{Body, Request, Response};
	use crate::conn_context::{ConnId, Transport};
	use crate::flow_log::{FlowLogEvent, FlowLogSink};

	// A runtime-free `AsyncRead + AsyncWrite` witness. `UnixStream::pair` and
	// `tokio::io::duplex` both require a running reactor; core tests
	// deliberately do not spin one up (16-crate-layout.md: no async-runtime
	// dep).
	struct NoopStream;

	impl AsyncRead for NoopStream {
		fn poll_read(
			self: Pin<&mut Self>,
			_cx: &mut Context<'_>,
			_buf: &mut ReadBuf<'_>,
		) -> Poll<io::Result<()>> {
			Poll::Ready(Ok(()))
		}
	}

	impl AsyncWrite for NoopStream {
		fn poll_write(
			self: Pin<&mut Self>,
			_cx: &mut Context<'_>,
			buf: &[u8],
		) -> Poll<io::Result<usize>> {
			Poll::Ready(Ok(buf.len()))
		}

		fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
			Poll::Ready(Ok(()))
		}

		fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
			Poll::Ready(Ok(()))
		}
	}

	struct NullSink;
	impl FlowLogSink for NullSink {
		fn emit(&self, _event: FlowLogEvent) {}
	}

	struct SynthOk;
	#[async_trait]
	impl L7Fetch for SynthOk {
		async fn fetch(
			&self,
			_req: Request,
			_conn: &Arc<ConnContext>,
			_ctx: &mut FlowCtx<'_>,
		) -> Result<L7FetchOutput, Error> {
			let resp: Response = http::Response::builder().status(200).body(Body::Empty).expect("build");
			Ok(L7FetchOutput::Response(resp))
		}
	}

	struct L4Nop;
	#[async_trait]
	impl L4Fetch for L4Nop {
		async fn fetch(
			&self,
			_l4: L4Conn,
			_conn: &Arc<ConnContext>,
			_ctx: &mut FlowCtx<'_>,
		) -> Result<Tunnel, Error> {
			let (tx, _rx) = oneshot::channel::<crate::middleware::CloseReason>();
			Ok(Tunnel {
				client: Box::pin(NoopStream) as Pin<Box<dyn AsyncReadWrite + Send>>,
				upstream: Box::pin(NoopStream) as Pin<Box<dyn AsyncReadWrite + Send>>,
				close_reason_tx: Some(tx),
			})
		}
	}

	fn assert_send<F: Send>(_: &F) {}

	fn make_conn_context() -> Arc<ConnContext> {
		let addr: SocketAddr = "127.0.0.1:0".parse().expect("parse addr");
		Arc::new(ConnContext {
			id: ConnId(0),
			remote: addr,
			local: addr,
			transport: Transport::Tcp,
			entered_at: Instant::now(),
			tls: Mutex::new(None),
			http_version: std::sync::OnceLock::new(),
			user: Mutex::new(http::Extensions::new()),
		})
	}

	#[test]
	fn async_read_write_blanket_accepts_async_io_type() {
		let _: Pin<Box<dyn AsyncReadWrite + Send>> = Box::pin(NoopStream);
	}

	#[test]
	fn l7_fetch_output_response_variant_constructs() {
		let resp: Response =
			http::Response::builder().status(200).body(Body::Empty).expect("build response");
		match L7FetchOutput::Response(resp) {
			L7FetchOutput::Response(_) => {}
			L7FetchOutput::Tunnel(_) => panic!("unexpected tunnel variant"),
		}
	}

	#[test]
	fn tunnel_builds_from_paired_async_io_streams() {
		let (tx, _rx) = oneshot::channel::<crate::middleware::CloseReason>();
		let tunnel = Tunnel {
			client: Box::pin(NoopStream) as Pin<Box<dyn AsyncReadWrite + Send>>,
			upstream: Box::pin(NoopStream) as Pin<Box<dyn AsyncReadWrite + Send>>,
			close_reason_tx: Some(tx),
		};
		let _ = L7FetchOutput::Tunnel(tunnel);
	}

	// `async_trait` makes `L7Fetch` and `L4Fetch` dyn-compatible. `FetchInst`
	// stores them as `Arc<dyn _>` per 05-terminator.md § _Trait surface_;
	// constructing that exact shape from a concrete impl is the contract we
	// guard here.

	#[test]
	fn l7_fetch_is_constructible_as_arc_dyn_send_sync() {
		let f: Arc<dyn L7Fetch + Send + Sync> = Arc::new(SynthOk);
		let _: Arc<dyn L7Fetch> = f;
	}

	#[test]
	fn l4_fetch_is_constructible_as_arc_dyn_send_sync() {
		let f: Arc<dyn L4Fetch + Send + Sync> = Arc::new(L4Nop);
		let _: Arc<dyn L4Fetch> = f;
	}

	#[test]
	fn l7_fetch_fetch_returns_send_future() {
		let f: Arc<dyn L7Fetch> = Arc::new(SynthOk);
		let conn = make_conn_context();
		let mut sink = NullSink;
		let mut span = tracing::Span::none();
		let cancel = CancellationToken::new();
		let mut ctx =
			FlowCtx { span: &mut span, log: &mut sink as &mut dyn FlowLogSink, cancel: &cancel };
		let req: Request = http::Request::builder().uri("/").body(Body::Empty).expect("build req");
		// Exact-type coercion — async_trait rewrites `fetch` to return
		// `Pin<Box<dyn Future + Send>>`; this binding fails to compile if the
		// future ever loses `Send`.
		let fut: Pin<Box<dyn Future<Output = Result<L7FetchOutput, Error>> + Send + '_>> =
			f.fetch(req, &conn, &mut ctx);
		assert_send(&fut);
		drop(fut);
	}

	#[test]
	fn fetch_kind_serde_round_trip_per_variant() {
		for k in [
			FetchKind::HttpProxy,
			FetchKind::HttpSynthesize,
			FetchKind::WebSocketUpgrade,
			FetchKind::L4Forward,
		] {
			let encoded = serde_json::to_string(&k).expect("serialize");
			let decoded: FetchKind = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, k);
		}
	}

	#[test]
	fn fetch_phase_serde_round_trip_per_variant() {
		for p in [FetchPhase::L4, FetchPhase::L7] {
			let encoded = serde_json::to_string(&p).expect("serialize");
			let decoded: FetchPhase = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, p);
		}
	}

	#[test]
	fn terminator_serde_round_trip_per_variant() {
		for t in [Terminator::WriteHttpResponse, Terminator::ByteTunnel] {
			let encoded = serde_json::to_string(&t).expect("serialize");
			let decoded: Terminator = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, t);
		}
	}

	#[test]
	fn fetch_output_modes_serde_round_trip_http_shapes() {
		// HttpProxy / HttpSynthesize: response-only.
		let http_only = FetchOutputModes { response: true, tunnel: false };
		// WebSocketUpgrade: both outputs, per the bi-outcome spec.
		let ws = FetchOutputModes { response: true, tunnel: true };
		// L4Forward: tunnel-only.
		let l4 = FetchOutputModes { response: false, tunnel: true };
		for modes in [http_only, ws, l4] {
			let encoded = serde_json::to_string(&modes).expect("serialize");
			let decoded: FetchOutputModes = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, modes);
		}
	}

	#[test]
	fn symbolic_fetch_ref_clone_preserves_fields() {
		let r = SymbolicFetchRef {
			kind: FetchKind::HttpProxy,
			args: json!({ "upstream": "127.0.0.1:8080" }),
		};
		let cloned = r.clone();
		assert_eq!(cloned.kind, r.kind);
		assert_eq!(cloned.args, r.args);
		// Debug must be derivable for diagnostics.
		let _ = format!("{r:?}");
	}

	#[test]
	fn symbolic_fetch_ref_accepts_each_kind() {
		for kind in [
			FetchKind::HttpProxy,
			FetchKind::HttpSynthesize,
			FetchKind::WebSocketUpgrade,
			FetchKind::L4Forward,
		] {
			let _ = SymbolicFetchRef { kind, args: serde_json::Value::Null };
		}
	}

	// Dry-run JSON wire-format contract: SymbolicFetchRef participates in
	// the compiled-form JSON per 02-flow.md § _The compiled form_. Both the
	// `kind` tag and the opaque `args` payload must round-trip.
	#[test]
	fn symbolic_fetch_ref_round_trip_preserves_kind_and_args() {
		let r = SymbolicFetchRef {
			kind: FetchKind::WebSocketUpgrade,
			args: json!({ "upstream": "127.0.0.1:9000" }),
		};
		let encoded = serde_json::to_string(&r).expect("serialize");
		let decoded: SymbolicFetchRef = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded.kind, r.kind);
		assert_eq!(decoded.args, r.args);
	}
}
