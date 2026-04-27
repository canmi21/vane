//! Boot health watchdog coverage. Two complementary scenarios:
//!
//! - **Total bind failure**: every listener loses to a pre-bound
//!   blocker. The watchdog must fire shutdown within the configured
//!   timeout and `vaned` must exit with a non-zero status.
//! - **Partial bind failure**: at least one listener succeeds. The
//!   watchdog must warn and leave the daemon running; the surviving
//!   listener stays observable through the mgmt plane.
//!
//! Each test runs `vaned` as a real subprocess so the
//! `BOOT_HEALTH_EXIT` → `ExitCode::FAILURE` path is exercised
//! end-to-end (a unit test could only assert the static was set).

use std::future::Future;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ChildStdout;
use std::process::Stdio;
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use vane_mgmt::UnixMgmtClient;
use vane_mgmt::verb::{NoArgs, StatsResult, VERB_STATS};

/// Poll `f` every 50ms until it returns `Some(v)`, then yield `v`.
/// Panics if `deadline` elapses without success — the call site should
/// pick a deadline that's a generous upper bound on the work being
/// observed (e.g. boot-health timeout + spawn jitter), not a tight
/// expectation. 50ms is the poll interval everywhere in this file —
/// shorter is CPU noise, longer leaves observable transitions
/// unnoticed.
async fn poll_until<T, F, Fut>(deadline: Duration, mut f: F) -> T
where
	F: FnMut() -> Fut,
	Fut: Future<Output = Option<T>>,
{
	let start = Instant::now();
	loop {
		if let Some(v) = f().await {
			return v;
		}
		assert!(
			start.elapsed() < deadline,
			"poll_until: deadline {deadline:?} exceeded without observing the expected condition",
		);
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
}

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

/// Reserve a TCP port by binding `127.0.0.1:0`, capture the assigned
/// address, and keep the listener alive for the caller to hold. Drop
/// the returned `TcpListener` to free the address.
fn reserve_port() -> (TcpListener, u16) {
	let l = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
	let port = l.local_addr().expect("local_addr").port();
	(l, port)
}

/// Same as `reserve_port` but releases the listener — the address is
/// then highly likely to be free for `vaned` to bind.
fn pick_free_port() -> u16 {
	let (l, port) = reserve_port();
	drop(l);
	port
}

fn write_rules(dir: &Path, rules_json: &str) {
	let rules = dir.join("rules");
	std::fs::create_dir_all(&rules).expect("rules/");
	std::fs::write(rules.join("site.json"), rules_json).expect("write rule");
}

fn rule_static_site(name: &str, port: u16, body: &str) -> String {
	format!(
		r#"{{
			"preset": "static_site",
			"name": "{name}",
			"listen": ["127.0.0.1:{port}"],
			"args": {{ "status": 200, "body": "{body}" }}
		}}"#
	)
}

/// Spawn `vaned` with the given rule set and boot-health timeout in
/// seconds. Stdout is piped so failure-path tests can grep for the
/// watchdog's "all listeners failed to bind" message —
/// `tracing_subscriber::fmt()` writes to stdout by default.
fn spawn_vaned(rules_json: &str, boot_health_timeout_secs: u64) -> Daemon {
	let tmp = tempfile::tempdir().expect("tempdir");
	let config_dir = tmp.path().to_path_buf();
	write_rules(&config_dir, rules_json);
	let socket = tmp.path().join("vaned.sock");

	let mut cmd = std::process::Command::cargo_bin("vaned").expect("locate vaned binary");
	cmd
		.arg("-c")
		.arg(&config_dir)
		.env("VANE_MGMT_UNIX", &socket)
		.env("VANE_BOOT_HEALTH_TIMEOUT_SECS", boot_health_timeout_secs.to_string())
		.env("RUST_LOG", "warn,vaned=info")
		.stdout(Stdio::piped())
		.stderr(Stdio::null());
	let child = cmd.spawn().expect("spawn vaned");
	Daemon { child, socket, _tmp: tmp }
}

fn drain_stdout(stdout: Option<ChildStdout>) -> String {
	let mut buf = String::new();
	if let Some(mut s) = stdout {
		let _ = s.read_to_string(&mut buf);
	}
	buf
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

#[tokio::test]
async fn bind_failure_exit_when_all_listeners_fail_to_bind() {
	// Reserve the only listen port the rule will reference, hold the
	// listener for the duration of the test, set a 3s health timeout
	// so the watchdog fires well before the bind retries naturally
	// exhaust (~26s worst case at MAX_BIND_ATTEMPTS = 10).
	let (_blocker, port) = reserve_port();
	let rules = format!(r#"{{ "rules": [{}] }}"#, rule_static_site("only", port, "x"));
	let mut d = spawn_vaned(&rules, 3);

	// Wait up to 10s for the daemon to give up and exit. Watchdog fires
	// at ~3s; shutdown drain budget is 30s but the listener never
	// bound, so there's nothing real to drain — exit should land in <5s.
	let deadline = Instant::now() + Duration::from_secs(15);
	let status = loop {
		if let Some(s) = d.child.try_wait().expect("try_wait") {
			break s;
		}
		assert!(Instant::now() < deadline, "vaned did not exit within 15s after total bind failure");
		tokio::time::sleep(Duration::from_millis(100)).await;
	};

	assert!(!status.success(), "vaned exited zero on total bind failure: {status:?}");
	// Drain stdout and confirm the watchdog actually fired — proves we
	// exited via the boot-health path rather than some other crash.
	let stdout = drain_stdout(d.child.stdout.take());
	assert!(
		stdout.contains("all listeners failed to bind"),
		"expected boot-health-timeout message, got stdout: {stdout}"
	);
}

#[tokio::test]
async fn bind_failure_partial_continues_serving() {
	// One blocked port + one free port. Watchdog observes 1 of 2 bound
	// at the timeout — partial path, daemon continues. We then call
	// the mgmt `stats` verb to confirm exactly one listener is bound.
	let (_blocker, blocked_port) = reserve_port();
	let free_port = pick_free_port();
	let rules = format!(
		r#"{{ "rules": [{}, {}] }}"#,
		rule_static_site("blocked", blocked_port, "x"),
		rule_static_site("free", free_port, "y"),
	);
	let boot_health_secs = 3;
	let d = spawn_vaned(&rules, boot_health_secs);
	wait_for_socket(&d.socket, Duration::from_secs(5));

	// Wait past the boot-health deadline so the watchdog has a chance
	// to fire (on partial bind it must log + continue, not exit). The
	// daemon-side `uptime_ms` field is the cleanest "watchdog has had
	// its chance" signal — it advances in real time regardless of
	// listener state. Polling on it (rather than sleeping a flat
	// 4 seconds) gives back a couple of fixed seconds per run while
	// still observing the post-deadline behaviour. 5s deadline is a
	// generous upper bound on `boot_health + spawn jitter`, not the
	// expected wallclock; the loop typically returns ~50ms after the
	// uptime crosses the threshold.
	let boot_health_ms = boot_health_secs * 1_000;
	let client = UnixMgmtClient::new(&d.socket);
	let stats: StatsResult = poll_until(Duration::from_secs(5), || async {
		let result: Result<StatsResult, _> = client.call(VERB_STATS, &NoArgs {}).await;
		match result {
			Ok(s) if s.uptime_ms >= boot_health_ms => Some(s),
			_ => None,
		}
	})
	.await;
	let bound: Vec<&str> =
		stats.listeners.iter().filter(|l| l.bound).map(|l| l.addr.as_str()).collect();
	assert_eq!(bound.len(), 1, "exactly one listener bound; got {bound:?}");
	assert!(
		bound[0].ends_with(&format!(":{free_port}")),
		"the bound listener is the free port; got {}",
		bound[0]
	);
}
