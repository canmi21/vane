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
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use vane_mgmt::UnixMgmtClient;
use vane_mgmt::verb::{
	ListConnectionsResult, NoArgs, PingResult, ReloadResult, ShutdownResult, StatsResult,
	VERB_LIST_CONNECTIONS, VERB_PING, VERB_RELOAD, VERB_SHUTDOWN, VERB_STATS,
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
async fn mgmt_ping_via_cli_pretty_prints_pong_line() {
	let d = spawn_daemon_with_rule(43_010, "v1");
	let mut cmd = std::process::Command::cargo_bin("vane").expect("vane binary");
	let output = cmd.arg("ping").arg("--socket").arg(&d.socket).output().expect("run vane ping");
	assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
	let stdout = String::from_utf8(output.stdout).expect("utf8");
	assert!(stdout.starts_with("pong (vaned "), "unexpected stdout: {stdout:?}");
}
