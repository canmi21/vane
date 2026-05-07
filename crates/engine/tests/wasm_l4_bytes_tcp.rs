//! `L4Bytes` plugin invocation on TCP / TLS listeners reads the same
//! peek buffer `L4Peek` sees — captured by the protocol-detection
//! prelude on `ConnContext.user` (spec/crates/engine.md § _Protocol detection_).
//! These tests exercise `dispatch_wasm` directly with a minimal mock
//! runtime so the assertions land squarely on the host-side
//! peek-buffer wiring rather than on a real wasm round-trip.

#![allow(clippy::too_many_lines)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::Mutex;
use vane_core::{
	BytesView, ConnContext, ConnId, Decision, L4BytesDecision, L4BytesInput, L4Conn, L4PeekDecision,
	L4PeekInput, L7RequestDecision, L7RequestInput, L7ResponseDecision, L7ResponseInput,
	MiddlewareKind, ModuleId, PeekResult, PluginError, PluginExport, PluginMetadata, Transport,
	WasmRuntime,
};
use vane_engine::executor::dispatch_wasm;
use vane_engine::flow_graph::WasmMiddleware;

// `WASM_BODY_LIMIT_L4` is a private constant inside the engine; the
// public contract is "first up-to-8 KiB chunk", and the executor caps
// at 64 KiB. We re-derive the cap here so the truncation test
// asserts the documented limit without exposing the constant.
const WASM_BODY_LIMIT_L4: usize = 64 * 1024;

// ─── minimal mock that records L4Bytes inputs ────────────────────────────────

struct RecordingRuntime {
	recorded: Mutex<Vec<BytesView>>,
	results: Mutex<Vec<Result<L4BytesDecision, PluginError>>>,
}

impl RecordingRuntime {
	fn with_results(results: Vec<Result<L4BytesDecision, PluginError>>) -> Arc<Self> {
		Arc::new(Self { recorded: Mutex::new(Vec::new()), results: Mutex::new(results) })
	}

	fn pop_recorded(&self) -> BytesView {
		let mut v = self.recorded.lock();
		assert_eq!(v.len(), 1, "expected exactly one invocation, got {}", v.len());
		v.remove(0)
	}
}

#[async_trait]
impl WasmRuntime for RecordingRuntime {
	async fn load_component(&self, _path: &Path) -> Result<Arc<PluginMetadata>, vane_core::Error> {
		unimplemented!("RecordingRuntime is mock-only")
	}

	async fn invoke_l4_peek(
		&self,
		_module_id: &ModuleId,
		_export_name: &str,
		_args_json: &str,
		_input: L4PeekInput,
	) -> Result<L4PeekDecision, PluginError> {
		unimplemented!("not exercised")
	}

	async fn invoke_l4_bytes(
		&self,
		_module_id: &ModuleId,
		_export_name: &str,
		_args_json: &str,
		input: L4BytesInput,
	) -> Result<L4BytesDecision, PluginError> {
		self.recorded.lock().push(input.bytes);
		self.results.lock().remove(0)
	}

	async fn invoke_l7_request(
		&self,
		_module_id: &ModuleId,
		_export_name: &str,
		_args_json: &str,
		_input: L7RequestInput,
	) -> Result<L7RequestDecision, PluginError> {
		unimplemented!("not exercised")
	}

	async fn invoke_l7_response(
		&self,
		_module_id: &ModuleId,
		_export_name: &str,
		_args_json: &str,
		_input: L7ResponseInput,
	) -> Result<L7ResponseDecision, PluginError> {
		unimplemented!("not exercised")
	}
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn make_conn() -> Arc<ConnContext> {
	let remote: SocketAddr = "127.0.0.1:55555".parse().expect("remote parse");
	let local: SocketAddr = "127.0.0.1:443".parse().expect("local parse");
	Arc::new(ConnContext {
		id: ConnId(1),
		remote,
		local,
		transport: Transport::Tcp,
		entered_at: Instant::now(),
		tls: Mutex::new(None),
		http_version: OnceLock::new(),
		user: Mutex::new(http::Extensions::new()),
	})
}

fn install_peek(conn: &ConnContext, buffer: Bytes) {
	let mut user = conn.user.lock();
	user.insert(PeekResult { buffer, detected: None, tls: None });
}

fn make_middleware(runtime: Arc<RecordingRuntime>) -> WasmMiddleware {
	let metadata = Arc::new(PluginMetadata {
		name: "mock".to_owned(),
		version: "0.1.0".to_owned(),
		abi_version: "0.1.0".to_owned(),
		exports: vec![PluginExport {
			name: "probe".to_owned(),
			kind: MiddlewareKind::L4Bytes,
			stateless: true,
			needs_body: false,
			inspects: vec![],
		}],
	});
	WasmMiddleware {
		module_id: ModuleId(Arc::from("/fake/plugin.wasm")),
		export_name: "probe".to_owned(),
		args_json: "{}".to_owned(),
		runtime: runtime as Arc<dyn WasmRuntime>,
		metadata,
	}
}

fn make_peeked_conn() -> L4Conn {
	// `L4Conn::Peeked` boxes any `AsyncReadWrite + Send`; a tokio
	// duplex pair is the lightest stand-in for a peeked TCP stream.
	// The dispatch path under test never reads from the stream — it
	// only inspects `ConnContext.user::<PeekResult>` — so the duplex
	// staying empty is fine.
	let (a, _b) = tokio::io::duplex(64);
	L4Conn::Peeked(Box::new(a))
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn l4_bytes_on_tcp_delivers_peek_buffer_bytes() {
	let runtime = RecordingRuntime::with_results(vec![Ok(L4BytesDecision::Continue)]);
	let mw = make_middleware(Arc::clone(&runtime));
	let conn = make_conn();
	install_peek(&conn, Bytes::from_static(b"hello-peek-bytes"));

	let mut l4 = Some(make_peeked_conn());
	let mut req = None;
	let mut resp = None;
	let result = dispatch_wasm(&mw, &mut l4, &mut req, &mut resp, &conn).await;
	assert!(matches!(result, Ok(Decision::Continue)), "Continue expected from L4Bytes mock");

	let recorded = runtime.pop_recorded();
	assert_eq!(recorded.data, b"hello-peek-bytes");
	assert!(!recorded.truncated, "small buffer must not be flagged truncated");
}

#[tokio::test]
async fn l4_bytes_on_tcp_truncates_at_wasm_body_limit() {
	let runtime = RecordingRuntime::with_results(vec![Ok(L4BytesDecision::Continue)]);
	let mw = make_middleware(Arc::clone(&runtime));
	let conn = make_conn();

	// Cap is 64 KiB; install a buffer above it to exercise truncation.
	let big = Bytes::from(vec![0xab_u8; WASM_BODY_LIMIT_L4 + 1024]);
	install_peek(&conn, big);

	let mut l4 = Some(make_peeked_conn());
	let mut req = None;
	let mut resp = None;
	let _ = dispatch_wasm(&mw, &mut l4, &mut req, &mut resp, &conn).await.expect("dispatch ok");

	let recorded = runtime.pop_recorded();
	assert_eq!(recorded.data.len(), WASM_BODY_LIMIT_L4, "must truncate at 64 KiB cap");
	assert!(recorded.truncated, "oversize buffer must set truncated=true");
	assert!(recorded.data.iter().all(|b| *b == 0xab), "truncated prefix must come from input");
}

#[tokio::test]
async fn l4_bytes_on_tcp_with_no_peek_buffer_delivers_empty_bytes() {
	let runtime = RecordingRuntime::with_results(vec![Ok(L4BytesDecision::Continue)]);
	let mw = make_middleware(Arc::clone(&runtime));
	let conn = make_conn();
	// Note: no install_peek — listener never ran the protocol-detection
	// prelude (needs_peek = false on this listener), and the spec says
	// plugins legitimately see an empty buffer in that case.

	let mut l4 = Some(make_peeked_conn());
	let mut req = None;
	let mut resp = None;
	let _ = dispatch_wasm(&mw, &mut l4, &mut req, &mut resp, &conn).await.expect("dispatch ok");

	let recorded = runtime.pop_recorded();
	assert!(recorded.data.is_empty(), "absent PeekResult must produce empty bytes");
	assert!(!recorded.truncated, "absent PeekResult must not set truncated");
}
