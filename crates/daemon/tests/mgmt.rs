//! End-to-end mgmt tests: spawn `vaned` as a subprocess with a custom
//! `VANE_MGMT_UNIX` socket and a tempdir config tree, then drive it
//! either with the typed `vane-mgmt` Rust client (faster, in-process
//! wire-shape coverage) or the `vane` CLI binary (covers the
//! pretty-print + JSON output paths).
//!
//! Per-test config dirs are isolated in `tempfile::tempdir()`; each
//! test picks a unique high port to avoid collisions even when run in
//! parallel.

use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use vane_mgmt::UnixMgmtClient;
use vane_mgmt::verb::{
	ListConnectionsResult, NoArgs, PingResult, ReloadResult, ShutdownResult, StatsResult,
	VERB_LIST_CONNECTIONS, VERB_PING, VERB_RELOAD, VERB_SHUTDOWN, VERB_STATS, VERB_TAIL_FLOW_LOG,
	VERB_TAIL_LOG,
};

struct Daemon {
	child: std::process::Child,
	socket: PathBuf,
	_tmp: tempfile::TempDir,
	config_dir: PathBuf,
}

impl Daemon {
	fn config_dir(&self) -> &Path {
		&self.config_dir
	}
}

impl Drop for Daemon {
	fn drop(&mut self) {
		// Best-effort kill — if a test failed mid-way, leave no zombie.
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

fn write_rule(dir: &Path, port: u16, body: &str) {
	let rules = dir.join("rules");
	std::fs::create_dir_all(&rules).expect("rules/");
	std::fs::write(
		rules.join("site.json"),
		format!(
			r#"{{
				"rules": [{{
					"preset": "static_site",
					"name": "site",
					"listen": ["127.0.0.1:{port}"],
					"args": {{ "status": 200, "body": "{body}" }}
				}}]
			}}"#
		),
	)
	.expect("write rule");
}

/// Spawn `vaned` with `VANE_MGMT_UNIX` pointing at a tempdir-local
/// socket. Polls the socket file for up to 5s before returning, so the
/// test can immediately call into mgmt.
fn spawn_daemon_with_rule(port: u16, body: &str) -> Daemon {
	let tmp = tempfile::tempdir().expect("tempdir");
	let config_dir = tmp.path().to_path_buf();
	write_rule(&config_dir, port, body);
	let socket = tmp.path().join("vaned.sock");

	let mut cmd = std::process::Command::cargo_bin("vaned").expect("locate vaned binary");
	cmd
		.arg("-c")
		.arg(&config_dir)
		.env("VANE_MGMT_UNIX", &socket)
		.env("RUST_LOG", "warn")
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	let child = cmd.spawn().expect("spawn vaned");

	wait_for_socket(&socket, Duration::from_secs(5));
	Daemon { child, socket, _tmp: tmp, config_dir }
}

fn wait_for_socket(path: &Path, timeout: Duration) {
	let deadline = Instant::now() + timeout;
	while Instant::now() < deadline {
		if path.exists() {
			return;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	panic!("daemon socket {} did not appear within {timeout:?}", path.display());
}

fn wait_for_listener(addr: std::net::SocketAddr, timeout: Duration) {
	let deadline = Instant::now() + timeout;
	while Instant::now() < deadline {
		if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
			return;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	panic!("listener {addr} did not bind within {timeout:?}");
}

#[tokio::test]
async fn mgmt_ping_returns_pong_via_typed_client() {
	let d = spawn_daemon_with_rule(43_001, "v1");
	let client = UnixMgmtClient::new(&d.socket);
	let r: PingResult = client.call(VERB_PING, &NoArgs {}).await.expect("ping");
	assert!(r.pong);
	assert_eq!(r.version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn mgmt_stats_returns_uptime_and_listener_status() {
	let d = spawn_daemon_with_rule(43_002, "v1");
	wait_for_listener("127.0.0.1:43002".parse().unwrap(), Duration::from_secs(3));

	let client = UnixMgmtClient::new(&d.socket);
	let r: StatsResult = client.call(VERB_STATS, &NoArgs {}).await.expect("stats");
	assert_eq!(r.graph_version_hash.len(), 64);
	assert_eq!(r.listeners.len(), 1);
	assert_eq!(r.listeners[0].addr, "127.0.0.1:43002");
	assert!(r.listeners[0].bound, "listener should be bound");
}

#[tokio::test]
async fn mgmt_reload_swaps_when_rules_change() {
	let d = spawn_daemon_with_rule(43_003, "v1");
	let client = UnixMgmtClient::new(&d.socket);
	// First reload — file unchanged → unchanged outcome.
	let r1: ReloadResult = client.call(VERB_RELOAD, &NoArgs {}).await.expect("reload 1");
	let h0 = match &r1 {
		ReloadResult::Unchanged { hash } | ReloadResult::Swapped { hash } => hash.clone(),
	};
	assert!(matches!(r1, ReloadResult::Unchanged { .. }), "first reload should be no-op");

	// Edit the rule body and reload.
	write_rule(d.config_dir(), 43_003, "v2");
	let r2: ReloadResult = client.call(VERB_RELOAD, &NoArgs {}).await.expect("reload 2");
	match r2 {
		ReloadResult::Swapped { hash } => assert_ne!(hash, h0),
		ReloadResult::Unchanged { .. } => panic!("expected swap after body change"),
	}
}

#[tokio::test]
async fn mgmt_get_active_config_returns_symbolic_graph_via_cli_json() {
	let d = spawn_daemon_with_rule(43_004, "v1");
	// Use the CLI binary so we cover the JSON-output path end-to-end.
	let mut cmd = std::process::Command::cargo_bin("vane").expect("vane binary");
	let output = cmd
		.arg("get-active-config")
		.arg("--socket")
		.arg(&d.socket)
		.output()
		.expect("run vane get-active-config");
	assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
	let stdout = String::from_utf8(output.stdout).expect("utf8");
	let value: serde_json::Value = serde_json::from_str(&stdout).expect("parse JSON");
	assert!(value.get("entries").is_some());
	assert!(value.get("nodes").is_some());
	assert!(value.get("meta").is_some());
}

#[tokio::test]
async fn mgmt_compile_dry_run_does_not_swap_active_graph() {
	let d = spawn_daemon_with_rule(43_005, "v1");
	let client = UnixMgmtClient::new(&d.socket);
	let stats_before: StatsResult = client.call(VERB_STATS, &NoArgs {}).await.expect("stats 1");

	// Build a sibling config directory with a different rule body and
	// dry-run-compile against it via the CLI.
	let tmp_b = tempfile::tempdir().unwrap();
	write_rule(tmp_b.path(), 43_006, "different");
	let mut cmd = std::process::Command::cargo_bin("vane").expect("vane binary");
	let output = cmd
		.arg("compile")
		.arg("--dry-run")
		.arg(tmp_b.path())
		.arg("--socket")
		.arg(&d.socket)
		.output()
		.expect("run vane compile");
	assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
	let stdout = String::from_utf8(output.stdout).expect("utf8");
	let value: serde_json::Value = serde_json::from_str(&stdout).expect("parse JSON");
	assert!(value.get("entries").is_some());

	// The active graph must be untouched.
	let stats_after: StatsResult = client.call(VERB_STATS, &NoArgs {}).await.expect("stats 2");
	assert_eq!(stats_before.graph_version_hash, stats_after.graph_version_hash);
}

#[tokio::test]
async fn mgmt_list_connections_returns_per_listener_summary() {
	let d = spawn_daemon_with_rule(43_007, "v1");
	wait_for_listener("127.0.0.1:43007".parse().unwrap(), Duration::from_secs(3));

	let client = UnixMgmtClient::new(&d.socket);
	let r: ListConnectionsResult =
		client.call(VERB_LIST_CONNECTIONS, &NoArgs {}).await.expect("list_connections");
	assert_eq!(r.listeners.len(), 1);
	assert_eq!(r.listeners[0].addr, "127.0.0.1:43007");
	assert!(r.listeners[0].bound);
	// `connections` is present (default-empty Vec). We don't assert
	// emptiness because the wait_for_listener probe leaves a brief
	// in-flight tail; a strong assertion would race the registry's
	// deregister guard. Per-conn detail under load is covered by
	// `mgmt_list_connections_returns_per_conn_detail_for_in_flight_connection`.
}

#[tokio::test]
async fn mgmt_list_connections_returns_per_conn_detail_for_in_flight_connection() {
	let d = spawn_daemon_with_rule(43_011, "v1");
	let listen_addr: std::net::SocketAddr = "127.0.0.1:43011".parse().unwrap();
	wait_for_listener(listen_addr, Duration::from_secs(3));

	// Hold a partial HTTP request open (same trick as the in-flight
	// counter test) so the connection stays in the registry while we
	// query the mgmt verb.
	let mut stream = TcpStream::connect(listen_addr).expect("connect");
	let client_local = stream.local_addr().expect("local_addr");
	stream.write_all(b"GET / HTTP/1.1\r\nHost: ").expect("partial write");
	tokio::time::sleep(Duration::from_millis(150)).await;

	// Use the CLI binary so we cover the JSON output path end-to-end.
	let mut cmd = std::process::Command::cargo_bin("vane").expect("vane binary");
	let output = cmd
		.arg("list-connections")
		.arg("--json")
		.arg("--socket")
		.arg(&d.socket)
		.output()
		.expect("run vane list-connections");
	assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
	let stdout = String::from_utf8(output.stdout).expect("utf8");
	let value: serde_json::Value = serde_json::from_str(&stdout).expect("parse JSON");
	let connections =
		value.get("connections").and_then(|v| v.as_array()).expect("connections array present");
	assert!(!connections.is_empty(), "at least one connection in registry");
	let entry = &connections[0];
	let remote = entry.get("remote").and_then(|v| v.as_str()).expect("remote string");
	assert_eq!(remote, client_local.to_string(), "remote matches client's local addr");
	let listener_addr = entry.get("listener_addr").and_then(|v| v.as_str()).expect("listener_addr");
	assert_eq!(listener_addr, "127.0.0.1:43011");
	let conn_id = entry.get("conn_id").and_then(|v| v.as_str()).expect("conn_id");
	assert_eq!(conn_id.len(), 16, "ConnId Display is 16 hex chars");
	assert!(entry.get("age_ms").is_some(), "age_ms field present");
	drop(stream);
}

#[tokio::test]
async fn mgmt_in_flight_count_increases_with_long_lived_connection() {
	// Static-site preset writes a response and closes; a long-lived
	// connection isn't naturally available without an l4_forward
	// upstream. Hold the connection open by writing a slow request and
	// observing the in-flight count before the daemon responds.
	let d = spawn_daemon_with_rule(43_008, "long-lived");
	let listen_addr: std::net::SocketAddr = "127.0.0.1:43008".parse().unwrap();
	wait_for_listener(listen_addr, Duration::from_secs(3));

	// Open a TCP stream and write only a partial HTTP request — the
	// daemon's per-conn handler is now blocked on `read` waiting for
	// the rest of the headers, which lands the connection in the
	// in-flight set.
	let mut stream = TcpStream::connect(listen_addr).expect("connect");
	stream.write_all(b"GET / HTTP/1.1\r\nHost: ").expect("partial write");
	// Give the daemon a moment to register the accept.
	tokio::time::sleep(Duration::from_millis(150)).await;

	let client = UnixMgmtClient::new(&d.socket);
	let stats: StatsResult = client.call(VERB_STATS, &NoArgs {}).await.expect("stats");
	assert!(
		stats.listeners[0].in_flight_count >= 1,
		"expected at least one in-flight connection, got {}",
		stats.listeners[0].in_flight_count
	);
	drop(stream);
}

#[tokio::test]
async fn mgmt_shutdown_drains_daemon() {
	let mut d = spawn_daemon_with_rule(43_009, "v1");
	let client = UnixMgmtClient::new(&d.socket);
	let r: ShutdownResult = client.call(VERB_SHUTDOWN, &NoArgs {}).await.expect("shutdown");
	assert!(r.draining);

	// Wait for the process to exit cleanly within a reasonable budget.
	let deadline = Instant::now() + Duration::from_secs(5);
	while Instant::now() < deadline {
		match d.child.try_wait().expect("try_wait") {
			Some(status) => {
				assert!(status.success(), "vaned exited non-zero: {status:?}");
				return;
			}
			None => std::thread::sleep(Duration::from_millis(50)),
		}
	}
	panic!("vaned did not exit within 5s after mgmt shutdown");
}

#[tokio::test]
async fn mgmt_tail_flow_log_streams_events_via_cli() {
	use std::io::{BufRead, BufReader as StdBufReader};
	use std::process::Stdio;

	let d = spawn_daemon_with_rule(43_012, "v1");
	let listen_addr: std::net::SocketAddr = "127.0.0.1:43012".parse().unwrap();
	wait_for_listener(listen_addr, Duration::from_secs(3));

	// Spawn the streaming CLI subprocess with stdout piped. Capture
	// stdout in a background thread so we can deadline-poll it from
	// the main test task without blocking on `Read` indefinitely.
	let mut tail = std::process::Command::cargo_bin("vane")
		.expect("vane binary")
		.arg("tail-flow-log")
		.arg("--json")
		.arg("--socket")
		.arg(&d.socket)
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.expect("spawn vane tail-flow-log");

	let stdout = tail.stdout.take().expect("piped stdout");
	let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
	std::thread::spawn(move || {
		let reader = StdBufReader::new(stdout);
		for line in reader.lines().map_while(Result::ok) {
			if line_tx.send(line).is_err() {
				break;
			}
		}
	});

	// Give the streaming subscriber a moment to land on the broadcast
	// channel before driving traffic. Without this, the request races
	// the subscriber registration and the events reach an empty channel.
	tokio::time::sleep(Duration::from_millis(300)).await;

	// Trigger one request — static_site preset emits at least one
	// FlowLogEvent (the per-request `Trajectory` summary) per request.
	let mut stream = TcpStream::connect(listen_addr).expect("connect");
	stream.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").expect("write");
	let mut sink = Vec::new();
	let _ = std::io::Read::read_to_end(&mut stream, &mut sink);

	// Deadline-poll for at least one NDJSON line carrying `kind`. 5s
	// budget covers slow CI; the typical wallclock is <100ms.
	let deadline = std::time::Instant::now() + Duration::from_secs(5);
	let mut got_trajectory = false;
	while std::time::Instant::now() < deadline {
		match line_rx.recv_timeout(Duration::from_millis(200)) {
			Ok(line) => {
				let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
				if let Some(kind) = v.get("kind").and_then(serde_json::Value::as_str)
					&& kind == "Trajectory"
				{
					got_trajectory = true;
					break;
				}
			}
			Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
			Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
		}
	}
	let _ = tail.kill();
	let _ = tail.wait();
	assert!(got_trajectory, "expected at least one Trajectory event from tail-flow-log stream");
}

#[tokio::test]
async fn mgmt_tail_log_streams_tracing_events_via_cli() {
	use std::io::{BufRead, BufReader as StdBufReader};
	use std::process::Stdio;

	let d = spawn_daemon_with_rule(43_014, "v1");

	// Spawn the streaming CLI subprocess piping stdout. Drain in a
	// background thread to avoid blocking on `Read` when polling.
	let mut tail = std::process::Command::cargo_bin("vane")
		.expect("vane binary")
		.arg("tail-log")
		.arg("--json")
		.arg("--socket")
		.arg(&d.socket)
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.expect("spawn vane tail-log");

	let stdout = tail.stdout.take().expect("piped stdout");
	let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
	std::thread::spawn(move || {
		let reader = StdBufReader::new(stdout);
		for line in reader.lines().map_while(Result::ok) {
			if line_tx.send(line).is_err() {
				break;
			}
		}
	});

	// Let the tail subscriber land on the broadcast channel before
	// triggering tracing emits — without this the reload events race
	// the subscribe and reach an empty channel.
	tokio::time::sleep(Duration::from_millis(300)).await;

	// Mutate the rule body and trigger a reload — the daemon emits
	// `tracing::info!("reloaded — flow graph swapped")` from the
	// watcher's handle_reload path. That's a stable anchor for the
	// test: we know the event will fire, we know it has a `level`
	// field, and it lands on the live broadcast channel.
	write_rule(d.config_dir(), 43_014, "v2");
	let client = UnixMgmtClient::new(&d.socket);
	let _: ReloadResult = client.call(VERB_RELOAD, &NoArgs {}).await.expect("reload");

	// Deadline-poll for any NDJSON line carrying a `level` field.
	let deadline = std::time::Instant::now() + Duration::from_secs(5);
	let mut got_event = false;
	while std::time::Instant::now() < deadline {
		match line_rx.recv_timeout(Duration::from_millis(200)) {
			Ok(line) => {
				let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
				if v.get("level").and_then(serde_json::Value::as_str).is_some() {
					got_event = true;
					break;
				}
			}
			Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
			Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
		}
	}
	let _ = tail.kill();
	let _ = tail.wait();
	assert!(got_event, "expected at least one tracing event with a level field");
}

#[tokio::test]
async fn mgmt_tail_log_via_typed_client_decodes_tracing_frame_shape() {
	// Subscribe via the typed Rust client and verify the wire shape
	// matches `TracingFrame` (t / level / target / message / fields).
	let d = spawn_daemon_with_rule(43_015, "v1");
	let socket = d.socket.clone();
	let config_dir = d.config_dir().to_path_buf();
	let frames: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
	let frames_for_task = Arc::clone(&frames);

	let stream_task = tokio::spawn(async move {
		let client = UnixMgmtClient::new(&socket);
		let _ = client
			.call_stream(VERB_TAIL_LOG, &NoArgs {}, |frame| {
				frames_for_task.lock().expect("lock").push(frame);
			})
			.await;
	});
	tokio::time::sleep(Duration::from_millis(300)).await;

	// Trigger a reload to anchor on a known emit.
	write_rule(&config_dir, 43_015, "v2");
	let client = UnixMgmtClient::new(&d.socket);
	let _: ReloadResult = client.call(VERB_RELOAD, &NoArgs {}).await.expect("reload");

	let deadline = std::time::Instant::now() + Duration::from_secs(5);
	loop {
		let any = !frames.lock().expect("lock").is_empty();
		if any || std::time::Instant::now() >= deadline {
			break;
		}
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
	stream_task.abort();
	let captured = frames.lock().expect("lock").clone();
	assert!(!captured.is_empty(), "expected at least one frame");
	let frame = &captured[0];
	assert!(frame.get("t").and_then(serde_json::Value::as_u64).is_some(), "t field present");
	assert!(frame.get("level").and_then(serde_json::Value::as_str).is_some(), "level field present");
	assert!(
		frame.get("target").and_then(serde_json::Value::as_str).is_some(),
		"target field present"
	);
	assert!(frame.get("message").is_some(), "message field present");
	assert!(frame.get("fields").is_some(), "fields field present");
}

#[tokio::test]
async fn mgmt_streaming_does_not_block_concurrent_one_shot_call() {
	// While one client is parked on a streaming verb (`tail_flow_log`)
	// holding its socket open, an independent client must still be able
	// to issue and receive a one-shot verb (`ping`) on a *separate*
	// socket. This is the per-conn-task isolation contract of the
	// server's accept loop.
	let d = spawn_daemon_with_rule(43_013, "v1");
	let listen_addr: std::net::SocketAddr = "127.0.0.1:43013".parse().unwrap();
	wait_for_listener(listen_addr, Duration::from_secs(3));

	let stream_socket = d.socket.clone();
	let stream_task = tokio::spawn(async move {
		let client = UnixMgmtClient::new(&stream_socket);
		// Park inside the streaming call. We never expect events here
		// (no request is fired against the data plane) — the future
		// runs until the test drops it.
		let _ = client.call_stream(VERB_TAIL_FLOW_LOG, &NoArgs {}, |_event| {}).await;
	});
	tokio::time::sleep(Duration::from_millis(200)).await;

	// One-shot ping on an independent socket must succeed promptly.
	let one_shot_client = UnixMgmtClient::new(&d.socket);
	let r = tokio::time::timeout(
		Duration::from_secs(2),
		one_shot_client.call::<_, PingResult>(VERB_PING, &NoArgs {}),
	)
	.await
	.expect("one-shot ping should not be blocked by streaming client")
	.expect("ping result");
	assert!(r.pong);
	stream_task.abort();
}

#[tokio::test]
async fn mgmt_ping_via_cli_pretty_prints_pong_line() {
	let d = spawn_daemon_with_rule(43_010, "v1");
	let mut cmd = std::process::Command::cargo_bin("vane").expect("vane binary");
	let output = cmd.arg("ping").arg("--socket").arg(&d.socket).output().expect("run vane ping");
	assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
	let stdout = String::from_utf8(output.stdout).expect("utf8");
	assert!(stdout.starts_with("pong (vaned "), "unexpected stdout: {stdout:?}");
}
