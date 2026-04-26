//! End-to-end coverage for the Unix mgmt transport: real socket file,
//! real `UnixListener`, real client connections.
//!
//! Sibling-crate consumers (vaned, vane CLI) rely on the public surface
//! exercised here — bind, perms, dispatch, cancel-and-cleanup, and
//! concurrent client connections.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use vane_mgmt::protocol::{Request, WireError, WireErrorKind};
use vane_mgmt::server::Handler;
use vane_mgmt::verb::{NoArgs, PingResult};
use vane_mgmt::{UnixMgmtClient, spawn_unix_server};

struct StubHandler;

#[async_trait]
impl Handler for StubHandler {
	async fn dispatch(&self, req: Request) -> Result<serde_json::Value, WireError> {
		match req.verb.as_str() {
			"ping" => Ok(serde_json::json!({ "pong": true, "version": "test-0.0.0" })),
			_ => Err(WireError {
				kind: WireErrorKind::UnknownVerb,
				message: format!("unknown {}", req.verb),
			}),
		}
	}
}

#[tokio::test]
async fn mgmt_unix_socket_round_trip_ping() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let socket = tmp.path().join("vaned.sock");
	let cancel = CancellationToken::new();
	let _server =
		spawn_unix_server(&socket, Arc::new(StubHandler), cancel.clone()).await.expect("spawn");

	// Verify the chmod 0600 contract — operator file-system perms are
	// the only access gate on the Unix transport.
	let mode = std::fs::metadata(&socket).expect("stat socket").permissions().mode() & 0o777;
	assert_eq!(mode, 0o600, "socket file must be chmod 0600");

	let client = UnixMgmtClient::new(&socket);
	let result: PingResult = client.call("ping", &NoArgs {}).await.expect("call");
	assert!(result.pong);
	assert_eq!(result.version, "test-0.0.0");

	cancel.cancel();
	tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn mgmt_unix_socket_unlinks_socket_on_cancel() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let socket = tmp.path().join("vaned.sock");
	let cancel = CancellationToken::new();
	let handle =
		spawn_unix_server(&socket, Arc::new(StubHandler), cancel.clone()).await.expect("spawn");

	assert!(socket.exists(), "socket file present after bind");
	cancel.cancel();
	let _ = handle.await;
	assert!(!socket.exists(), "socket file removed on cancel");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mgmt_unix_socket_handles_concurrent_clients() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let socket = tmp.path().join("vaned.sock");
	let cancel = CancellationToken::new();
	let _server =
		spawn_unix_server(&socket, Arc::new(StubHandler), cancel.clone()).await.expect("spawn");

	let mut joins = Vec::new();
	for _ in 0..8 {
		let socket = socket.clone();
		joins.push(tokio::spawn(async move {
			let client = UnixMgmtClient::new(&socket);
			client.call::<_, PingResult>("ping", &NoArgs {}).await
		}));
	}
	for j in joins {
		let res = j.await.expect("join").expect("call");
		assert!(res.pong);
	}
	cancel.cancel();
}
