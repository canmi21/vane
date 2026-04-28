//! End-to-end tests for the `get_metrics` mgmt verb.
//!
//! Each test spawns a real `vaned` subprocess, drives the mgmt channel,
//! and asserts on the Prometheus text or JSON output.

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use vane_mgmt::UnixMgmtClient;
use vane_mgmt::verb::{GetMetricsArgs, GetMetricsResult, VERB_GET_METRICS};
use vane_mgmt::{MgmtClientError, WireErrorKind};

// ── helpers ──────────────────────────────────────────────────────────────

struct Daemon {
	child: std::process::Child,
	socket: PathBuf,
	_tmp: tempfile::TempDir,
}

impl Drop for Daemon {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

fn write_static_rule(dir: &Path, port: u16) {
	let rules = dir.join("rules");
	fs::create_dir_all(&rules).expect("rules/");
	fs::write(
        rules.join("site.json"),
        format!(
            r#"{{"rules":[{{"preset":"static_site","name":"site","listen":["127.0.0.1:{port}"],"args":{{"status":200,"body":"ok"}}}}]}}"#
        ),
    )
    .expect("write rule");
}

fn write_rate_limit_rule(dir: &Path, listen_port: u16, upstream_port: u16) {
	let rules = dir.join("rules");
	fs::create_dir_all(&rules).expect("rules/");
	fs::write(
		rules.join("rl.json"),
		format!(
			r#"{{"rules":[{{"preset":"reverse_proxy","name":"rl","listen":["127.0.0.1:{listen_port}"],"args":{{"upstream":"127.0.0.1:{upstream_port}","rate_limit":{{"rate":1,"burst":1,"window":"60s"}}}}}}]}}"#
		),
	)
	.expect("write rate-limit rule");
}

fn spawn_daemon(rule_dir: &Path) -> Daemon {
	let tmp = tempfile::tempdir().expect("tempdir");
	let socket = tmp.path().join("vaned.sock");
	let mut cmd = std::process::Command::cargo_bin("vaned").expect("locate vaned binary");
	cmd
		.arg("-c")
		.arg(rule_dir)
		.env("VANE_MGMT_UNIX", &socket)
		.env("VANE_MGMT_HTTP_PORT", "")
		.env("RUST_LOG", "warn")
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	let child = cmd.spawn().expect("spawn vaned");
	wait_for_socket(&socket, Duration::from_secs(5));
	Daemon { child, socket, _tmp: tmp }
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

// ── tests ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_metrics_default_format_returns_prometheus_text() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_static_rule(tmp.path(), 44_001);
	let d = spawn_daemon(tmp.path());
	let client = UnixMgmtClient::new(&d.socket);

	let r: GetMetricsResult =
		client.call(VERB_GET_METRICS, &GetMetricsArgs { format: None }).await.expect("get_metrics");
	match r {
		GetMetricsResult::Prometheus { body } => {
			// The recorder is installed; body may be empty but must not error.
			assert!(!body.contains("error"), "prometheus body must not contain 'error': {body}");
		}
		GetMetricsResult::Json { .. } => {
			panic!("default format should be Prometheus, got Json");
		}
	}
}

#[tokio::test]
async fn get_metrics_explicit_prometheus() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_static_rule(tmp.path(), 44_002);
	let d = spawn_daemon(tmp.path());
	let client = UnixMgmtClient::new(&d.socket);

	let r: GetMetricsResult = client
		.call(VERB_GET_METRICS, &GetMetricsArgs { format: Some("prometheus".to_string()) })
		.await
		.expect("get_metrics");
	assert!(
		matches!(r, GetMetricsResult::Prometheus { .. }),
		"explicit prometheus format must return Prometheus variant",
	);
}

#[tokio::test]
async fn get_metrics_json_returns_structured() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_static_rule(tmp.path(), 44_003);
	let d = spawn_daemon(tmp.path());
	let client = UnixMgmtClient::new(&d.socket);

	let r: GetMetricsResult = client
		.call(VERB_GET_METRICS, &GetMetricsArgs { format: Some("json".to_string()) })
		.await
		.expect("get_metrics json");
	match r {
		GetMetricsResult::Json { metrics } => {
			assert!(metrics.get("samples").is_some(), "JSON must have `samples` key");
			assert!(metrics.get("docs").is_some(), "JSON must have `docs` key");
		}
		GetMetricsResult::Prometheus { .. } => {
			panic!("format=json must return Json variant");
		}
	}
}

#[tokio::test]
async fn get_metrics_rejects_unknown_format() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_static_rule(tmp.path(), 44_004);
	let d = spawn_daemon(tmp.path());
	let client = UnixMgmtClient::new(&d.socket);

	let err = client
		.call::<_, GetMetricsResult>(
			VERB_GET_METRICS,
			&GetMetricsArgs { format: Some("yaml".to_string()) },
		)
		.await
		.expect_err("unknown format must error");
	match err {
		MgmtClientError::Server(w) => {
			assert_eq!(w.kind, WireErrorKind::BadArgs, "must be BadArgs, got: {:?}", w.kind);
		}
		other => panic!("expected Server(BadArgs), got {other:?}"),
	}
}

#[tokio::test]
async fn get_metrics_after_real_traffic_shows_requests_total() {
	use std::io::{Read as _, Write as _};
	let tmp = tempfile::tempdir().expect("tempdir");
	let port: u16 = 44_005;
	write_static_rule(tmp.path(), port);
	let d = spawn_daemon(tmp.path());
	let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
	wait_for_listener(addr, Duration::from_secs(5));

	// Send a minimal HTTP request to trigger handle_connection.
	let mut conn = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
	conn.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n").expect("write");
	let mut buf = Vec::new();
	let _ = conn.read_to_end(&mut buf);

	// Give the daemon a moment to update the counter.
	std::thread::sleep(Duration::from_millis(100));

	let client = UnixMgmtClient::new(&d.socket);
	let r: GetMetricsResult =
		client.call(VERB_GET_METRICS, &GetMetricsArgs { format: None }).await.expect("get_metrics");
	let GetMetricsResult::Prometheus { body } = r else { panic!("expected Prometheus format") };
	// The exporter converts 'vane.requests.total' → 'vane_requests_total_total'
	// or 'vane_requests_total'. Accept either naming convention.
	assert!(
		body.contains("vane_requests_total"),
		"prometheus body must contain vane_requests_total after traffic; got:\n{body}",
	);
}

#[tokio::test]
async fn get_metrics_emits_security_limit_hit_after_rate_limit() {
	use std::io::{Read as _, Write as _};

	let tmp = tempfile::tempdir().expect("tempdir");
	let listen_port: u16 = 44_006;
	// Point upstream at a port that is almost certainly closed — we only
	// need the rate-limit deny to fire before the proxy attempt.
	let upstream_port: u16 = 44_099;
	write_rate_limit_rule(tmp.path(), listen_port, upstream_port);
	let d = spawn_daemon(tmp.path());
	let addr: std::net::SocketAddr = format!("127.0.0.1:{listen_port}").parse().unwrap();
	wait_for_listener(addr, Duration::from_secs(5));

	// First request consumes the burst=1 token (allowed).
	let send_http = |addr: std::net::SocketAddr| {
		let mut c = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
		c.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n").expect("write");
		let mut buf = Vec::new();
		let _ = c.read_to_end(&mut buf);
		buf
	};
	send_http(addr);
	// Second request should hit the rate limit (token bucket exhausted).
	let resp = send_http(addr);
	assert!(resp.windows(3).any(|w| w == b"429"), "second request must be rate-limited (429)");

	std::thread::sleep(Duration::from_millis(100));

	let client = UnixMgmtClient::new(&d.socket);
	let r: GetMetricsResult =
		client.call(VERB_GET_METRICS, &GetMetricsArgs { format: None }).await.expect("get_metrics");
	let GetMetricsResult::Prometheus { body } = r else { panic!("expected Prometheus format") };
	assert!(
		body.contains("vane_security_limit_hit_total"),
		"prometheus body must contain vane_security_limit_hit_total after rate-limit deny;\ngot:\n{body}",
	);
}
