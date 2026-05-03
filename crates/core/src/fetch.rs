use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

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
		ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error>;
}

#[async_trait]
pub trait L4Fetch: Send + Sync {
	async fn fetch(
		&self,
		l4: L4Conn,
		conn: &Arc<ConnContext>,
		ctx: &mut FlowCtx,
	) -> Result<Tunnel, Error>;
}

pub enum L7FetchOutput {
	Response(Response),
	Tunnel(Tunnel),
}

/// Bridge between the executor's `ByteTunnel` arm and a fetch's chosen
/// transport. `Bidi` is the stream-pair shape that
/// `tokio::io::copy_bidirectional` consumes — covers TCP forward, TLS
/// passthrough, and the H1 WebSocket post-upgrade path. `Udp` is the
/// session-driven shape: the fetch has already spawned the per-5-tuple
/// forwarder task; the executor's role degenerates to awaiting
/// `join` so `ConnContext` cleanup runs at the right moment.
///
/// See `spec/architecture/06-l4.md` § _`udp_dispatch`_ for the UDP
/// session lifecycle and § _`l4_forward`_ for the TCP arm.
pub enum Tunnel {
	Bidi {
		client: Box<dyn AsyncReadWrite + Send>,
		upstream: Box<dyn AsyncReadWrite + Send>,
		close_reason_tx: Option<oneshot::Sender<CloseReason>>,
	},
	Udp(UdpTunnel),
}

/// Handle for an in-flight UDP session whose forwarder task already
/// runs in the background. `join` resolves with the session's terminal
/// `CloseReason` after the forwarder unwinds (idle timeout, peer EOF,
/// listener cancellation, or upstream send failure). `cancel`
/// propagates the executor's cancel token (typically the listener's
/// `force_cancel`) into the forwarder loop. The fetch is responsible
/// for inserting the matching `DispatchTable` entry on session start
/// and removing it as the `join` future resolves — vane-core stays
/// agnostic about the table type.
pub struct UdpTunnel {
	pub join: Pin<Box<dyn Future<Output = CloseReason> + Send>>,
	pub cancel: CancellationToken,
}

// `Unpin` is in the trait bound so `tokio::io::copy_bidirectional`
// (used by `Terminator::ByteTunnel` in the engine) can drive the streams
// directly. `TcpStream` / `UnixStream` / `tokio::io::DuplexStream` /
// `tokio_rustls::TlsStream<T: Unpin>` all satisfy it.
pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin {}
impl<T: AsyncRead + AsyncWrite + Unpin + ?Sized> AsyncReadWrite for T {}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum FetchKind {
	HttpProxy,
	HttpSynthesize,
	WebSocketUpgrade,
	L4Forward,
}

impl FetchKind {
	/// Authoritative fetch-phase classification. The lower pass uses
	/// this to derive each listener's [`crate::ir::ListenerKind`] from
	/// the set of reachable terminators per entry; new fetch kinds
	/// pick their phase here so the derivation table in
	/// `06-l4.md` § _Listener kind derivation_ stays single-source.
	#[must_use]
	pub const fn phase(self) -> FetchPhase {
		match self {
			Self::L4Forward => FetchPhase::L4,
			Self::HttpProxy | Self::HttpSynthesize | Self::WebSocketUpgrade => FetchPhase::L7,
		}
	}
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
	/// `true` iff this fetch's retry policy is `buffering: "force"`
	/// with `max_attempts > 1`. Drives `collect_body_before`
	/// placement on the fetch node in the lower pass; the full
	/// `RetryPolicy` lives in the engine's factory layer. See
	/// `spec/architecture/05-terminator.md` § _Retry buffering_.
	#[serde(default)]
	pub retry_buffer_required: bool,
	/// Per-rule TLS 1.3 0-RTT acceptance, lifted off the parent rule's
	/// `allow_zero_rtt` field by the lower pass. `Some(true)` means the
	/// rule opts into accepting requests that arrived as 0-RTT data;
	/// `Some(false)` means a 0-RTT request matched against this rule
	/// must receive a synthetic 425 Too Early instead of being handed
	/// to the terminator. `None` means the rule's listener is not
	/// TLS-terminating L7 (the runtime check is unreachable; this
	/// arm exists so non-TLS fixtures need not populate the field).
	/// See `spec/architecture/08-tls.md` § _TLS 1.3 0-RTT (early data)_
	/// _Runtime flow_.
	#[serde(default)]
	pub allow_zero_rtt: Option<bool>,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Terminator {
	WriteHttpResponse,
	ByteTunnel,
	Close,
}

/// Per-call limits forwarded to `HttpFetchBackend::fetch` alongside the request.
///
/// All fields mirror the WASM ABI's `http-fetch-request` per-call knobs
/// (see `spec/wasm-abi.md` § _Host functions_). The backend applies the
/// three-level fallback: per-call override → plugin config default →
/// daemon default (30 s timeout, 5 redirects, TLS verified).
#[derive(Clone, Debug)]
pub struct HttpFetchLimits {
	pub max_body_bytes: u64,
	pub timeout_ms: Option<u32>,
	pub follow_redirects: Option<u32>,
	pub allow_insecure: bool,
}

impl Default for HttpFetchLimits {
	fn default() -> Self {
		Self {
			max_body_bytes: 1024 * 1024,
			timeout_ms: None,
			follow_redirects: Some(5),
			allow_insecure: false,
		}
	}
}

/// Outbound HTTP request data passed to `HttpFetchBackend`.
///
/// Mirrors the WIT `http-fetch-request` record exactly.
#[derive(Debug)]
pub struct HttpFetchRequest {
	pub method: String,
	pub url: String,
	pub headers: Vec<(String, String)>,
	pub body: Vec<u8>,
	pub timeout_ms: Option<u32>,
	pub follow_redirects: Option<u32>,
	pub verify_tls: Option<bool>,
}

/// Response returned by `HttpFetchBackend`.
#[derive(Debug)]
pub struct HttpFetchResponse {
	pub status: u16,
	pub headers: Vec<(String, String)>,
	pub body: Vec<u8>,
}

/// Typed transport error from `HttpFetchBackend`.
///
/// Mirrors the WIT `net-error` variant exactly.
#[derive(Debug, thiserror::Error)]
pub enum HttpFetchError {
	#[error("dns failure: {0}")]
	DnsFailure(String),
	#[error("connection refused")]
	ConnectionRefused,
	#[error("timeout")]
	Timeout,
	#[error("tls error: {0}")]
	TlsError(String),
	#[error("pool exhausted")]
	PoolExhausted,
	#[error("body too large")]
	BodyTooLarge,
	#[error("not allowed: {0}")]
	NotAllowed(String),
	#[error("insecure rejected")]
	InsecureRejected,
	#[error("internal: {0}")]
	Internal(String),
}

/// Backend trait for outbound HTTP from WASM plugins.
///
/// Declared in `vane-core` so `vane-wasm` can call it without depending on
/// `vane-engine`. `vane-engine` provides the concrete impl wrapping `TcpPool`.
/// Tests substitute a mock. See `spec/architecture/11-wasm.md` § _http-fetch policy_.
#[async_trait]
pub trait HttpFetchBackend: Send + Sync {
	async fn fetch(
		&self,
		req: HttpFetchRequest,
		limits: HttpFetchLimits,
	) -> Result<HttpFetchResponse, HttpFetchError>;
}

#[cfg(test)]
mod tests {
	use std::future::Future;
	use std::io;
	use std::net::SocketAddr;
	use std::pin::Pin;
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
			_ctx: &mut FlowCtx,
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
			_ctx: &mut FlowCtx,
		) -> Result<Tunnel, Error> {
			let (tx, _rx) = oneshot::channel::<crate::middleware::CloseReason>();
			Ok(Tunnel::Bidi {
				client: Box::new(NoopStream) as Box<dyn AsyncReadWrite + Send>,
				upstream: Box::new(NoopStream) as Box<dyn AsyncReadWrite + Send>,
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
		let _: Box<dyn AsyncReadWrite + Send> = Box::new(NoopStream);
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
	fn tunnel_bidi_builds_from_paired_async_io_streams() {
		let (tx, _rx) = oneshot::channel::<crate::middleware::CloseReason>();
		let tunnel = Tunnel::Bidi {
			client: Box::new(NoopStream) as Box<dyn AsyncReadWrite + Send>,
			upstream: Box::new(NoopStream) as Box<dyn AsyncReadWrite + Send>,
			close_reason_tx: Some(tx),
		};
		let _ = L7FetchOutput::Tunnel(tunnel);
	}

	#[test]
	fn tunnel_udp_builds_from_join_future_and_cancel_token() {
		let cancel = CancellationToken::new();
		let join: Pin<Box<dyn Future<Output = CloseReason> + Send>> =
			Box::pin(async move { CloseReason::Graceful });
		let tunnel = Tunnel::Udp(UdpTunnel { join, cancel });
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
		let mut ctx = FlowCtx {
			span: tracing::Span::none(),
			log: Arc::new(NullSink),
			cancel: CancellationToken::new(),
			verbosity: crate::flow_log::FlowLogVerbosity::Trajectory,
			trajectory: crate::flow_log::TrajectoryBuilder::new(conn.id, crate::ir::NodeId::new(0), 0),
		};
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
			retry_buffer_required: false,
			allow_zero_rtt: None,
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
			let _ = SymbolicFetchRef {
				kind,
				args: serde_json::Value::Null,
				retry_buffer_required: false,
				allow_zero_rtt: None,
			};
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
			retry_buffer_required: false,
			allow_zero_rtt: None,
		};
		let encoded = serde_json::to_string(&r).expect("serialize");
		let decoded: SymbolicFetchRef = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded.kind, r.kind);
		assert_eq!(decoded.args, r.args);
	}
}
