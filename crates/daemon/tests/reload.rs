//! Integration tests for vaned's hot-reload path. Spawn the real
//! binary against a tempdir-backed config tree, edit / delete /
//! corrupt a rule file at runtime, and verify how the active graph
//! and live traffic respond.
//!
//! Each test owns its own ephemeral port + tempdir so they parallelize
//! cleanly. Readiness is detected by TCP-connect polling, the same
//! pattern as `tests/boot.rs` (tracing-subscriber block-buffers when
//! stderr is not a tty, so log-line parsing is unreliable here).

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

fn write_rule(dir: &Path, name: &str, body: &str) {
	let rules = dir.join("rules");
	fs::create_dir_all(&rules).expect("create rules/");
	fs::write(rules.join(name), body).expect("write rule");
}

fn ephemeral_port() -> u16 {
	let l = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
	let port = l.local_addr().expect("local addr").port();
	drop(l);
	port
}

fn spawn_vaned(dir: &Path) -> Child {
	let cmd = assert_cmd::Command::cargo_bin("vaned").expect("locate vaned bin");
	let bin = cmd.get_program().to_owned();
	Command::new(bin)
		.arg("-c")
		.arg(dir)
		// Disable the HTTP mgmt transport so parallel test daemons
		// don't fight over port 3333.
		.env("VANE_MGMT_HTTP_PORT", "")
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.expect("spawn vaned")
}

fn wait_for_port_open(port: u16, timeout: Duration) {
	let deadline = Instant::now() + timeout;
	while Instant::now() < deadline {
		if TcpStream::connect_timeout(
			&format!("127.0.0.1:{port}").parse().expect("addr"),
			Duration::from_millis(100),
		)
		.is_ok()
		{
			return;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	panic!("port {port} did not become reachable within {timeout:?}");
}

fn kill_signal(child: &Child, sig: Signal) {
	let pid_raw: i32 = child.id().try_into().expect("child pid fits i32");
	kill(Pid::from_raw(pid_raw), sig).expect("nix::kill");
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
	let deadline = Instant::now() + timeout;
	loop {
		match child.try_wait() {
			Ok(Some(status)) => return status,
			Ok(None) => {
				if Instant::now() >= deadline {
					let _ = child.kill();
					panic!("daemon did not exit within {timeout:?}");
				}
				std::thread::sleep(Duration::from_millis(50));
			}
			Err(e) => panic!("try_wait: {e}"),
		}
	}
}

/// Open a fresh TCP connection to `127.0.0.1:port`, send a single
/// `GET / HTTP/1.1` request, and return the full response string. Each
/// call uses a new connection so per-accept entry lookup runs against
/// the active graph for every probe.
fn http_get(port: u16) -> std::io::Result<String> {
	let mut stream = TcpStream::connect_timeout(
		&format!("127.0.0.1:{port}").parse().expect("addr"),
		Duration::from_secs(1),
	)?;
	stream.set_read_timeout(Some(Duration::from_secs(2)))?;
	stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
	let mut buf = String::new();
	stream.read_to_string(&mut buf)?;
	Ok(buf)
}

/// Static-site rule fixture. Plain JSON; the watcher's debounce
/// detects the modify and the reload pipeline rebuilds with a fresh
/// `version_hash`. The rule name embeds the port so multiple fixture
/// rules in the same config tree don't collide on the merge stage's
/// duplicate-name check.
fn static_site_rule(port: u16, body: &str) -> String {
	format!(
		r#"{{
			"rules": [{{
				"preset": "static_site",
				"name": "site_{port}",
				"listen": ["127.0.0.1:{port}"],
				"args": {{ "status": 200, "body": "{body}" }}
			}}]
		}}"#
	)
}

/// Hard ceiling for the `wait_until` polling loop. The real path
/// (250 ms watcher debounce + recompile + `ArcSwap` + listener
/// reconcile) finishes in 1-2 s solo; the binary is pinned to a
/// single concurrent test under nextest (see `.config/nextest.toml`
/// `[test-groups.reload-serial]`). The remaining variance comes
/// from the broader workspace contending for CPU — `reload_adds_*`
/// in particular has to bind a fresh listener post-reload, and that
/// occasionally pushes total wall-clock past 10 s under heavy load.
/// 15 s gives enough head-room without masking real bugs (any reload
/// that takes more than 15 s is broken, not just slow).
const RELOAD_BUDGET: Duration = Duration::from_secs(15);

/// Poll `predicate` every 50ms until it returns true, or panic at
/// `RELOAD_BUDGET`.
fn wait_until(mut predicate: impl FnMut() -> bool, msg: &str) {
	let deadline = Instant::now() + RELOAD_BUDGET;
	while Instant::now() < deadline {
		if predicate() {
			return;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	panic!("{msg}");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn reload_routes_through_new_rule_after_edit() {
	// Verifies the per-accept entry lookup. If the listener accept loop
	// baked in a boot-time `NodeId`, the post-reload graph (which has
	// reassigned slab indices) would either route to the old logical
	// entry (wrong body) or panic on out-of-bounds. The fix returns
	// "v2" cleanly.
	let tmp = tempfile::tempdir().expect("tempdir");
	let port = ephemeral_port();
	write_rule(tmp.path(), "site.json", &static_site_rule(port, "v1"));

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(port, Duration::from_secs(10));

	let resp = http_get(port).expect("v1 request");
	assert!(resp.contains("v1"), "expected v1, got {resp:?}");

	// Edit the rule body in place.
	write_rule(tmp.path(), "site.json", &static_site_rule(port, "v2"));

	wait_until(
		|| http_get(port).is_ok_and(|r| r.contains("v2")),
		"reloaded graph never started serving v2",
	);

	kill_signal(&child, Signal::SIGTERM);
	let status = wait_with_timeout(&mut child, Duration::from_secs(5));
	assert!(status.success(), "SIGTERM exit: {status:?}");
}

#[test]
fn reload_with_deleted_rule_drops_new_connections() {
	// After the only rule's file is removed, the active graph has no
	// entry for the still-bound listener. New connections should be
	// dropped immediately (TCP RST) rather than panic or route into the
	// stale graph.
	let tmp = tempfile::tempdir().expect("tempdir");
	let port = ephemeral_port();
	write_rule(tmp.path(), "site.json", &static_site_rule(port, "v1"));

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(port, Duration::from_secs(10));
	assert!(http_get(port).expect("v1 request").contains("v1"));

	fs::remove_file(tmp.path().join("rules").join("site.json")).expect("rm rule");

	// Connection attempts must yield non-200 — either connect-error,
	// EOF before headers, or empty body. The listener may keep
	// accepting (no listener-set diff yet) but should drop streams.
	wait_until(
		|| http_get(port).map_or_else(|_| true, |r| !r.contains("HTTP/1.1 200")),
		"connections continued returning 200 after rule removal",
	);

	kill_signal(&child, Signal::SIGTERM);
	let status = wait_with_timeout(&mut child, Duration::from_secs(5));
	assert!(status.success(), "SIGTERM exit: {status:?}");
}

#[test]
fn reload_adds_new_listen_port_serves_traffic() {
	// Start with one rule on port_a; verify "a". Drop a second rule
	// file introducing port_b. After the watcher debounces and
	// reconcile binds port_b, both ports must serve their respective
	// bodies.
	let tmp = tempfile::tempdir().expect("tempdir");
	let port_a = ephemeral_port();
	let port_b = ephemeral_port();
	assert_ne!(port_a, port_b);

	write_rule(tmp.path(), "a.json", &static_site_rule(port_a, "a"));

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(port_a, Duration::from_secs(10));
	assert!(http_get(port_a).expect("a").contains('a'));

	// Add a second rule file. Watcher → reload → reconcile binds port_b.
	write_rule(tmp.path(), "b.json", &static_site_rule(port_b, "b"));

	wait_until(
		|| http_get(port_b).is_ok_and(|r| r.contains('b')),
		"reconcile never bound the new listen port",
	);
	// Original port still serves.
	assert!(http_get(port_a).expect("a after reconcile").contains('a'));

	kill_signal(&child, Signal::SIGTERM);
	let status = wait_with_timeout(&mut child, Duration::from_secs(5));
	assert!(status.success(), "SIGTERM exit: {status:?}");
}

#[test]
fn reload_removes_listen_port_drops_new_connections() {
	// Start with two rules / two ports; verify both serve. Delete the
	// second rule. After the watcher debounces and reconcile drains
	// port_b, port_a must keep serving while port_b refuses new
	// connections.
	let tmp = tempfile::tempdir().expect("tempdir");
	let port_a = ephemeral_port();
	let port_b = ephemeral_port();
	assert_ne!(port_a, port_b);

	write_rule(tmp.path(), "a.json", &static_site_rule(port_a, "a"));
	write_rule(tmp.path(), "b.json", &static_site_rule(port_b, "b"));

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(port_a, Duration::from_secs(10));
	wait_for_port_open(port_b, Duration::from_secs(10));
	assert!(http_get(port_a).expect("a").contains('a'));
	assert!(http_get(port_b).expect("b").contains('b'));

	// Remove b.json; reconcile background-drains port_b's listener.
	fs::remove_file(tmp.path().join("rules").join("b.json")).expect("rm b");

	// port_b must stop returning 200; the underlying socket eventually
	// closes (background drain). Either connect-refused or empty body is
	// acceptable.
	wait_until(
		|| http_get(port_b).map_or_else(|_| true, |r| !r.contains("HTTP/1.1 200")),
		"port_b kept serving after rule removal",
	);
	// port_a survives untouched.
	assert!(http_get(port_a).expect("a after reconcile").contains('a'));

	kill_signal(&child, Signal::SIGTERM);
	let status = wait_with_timeout(&mut child, Duration::from_secs(5));
	assert!(status.success(), "SIGTERM exit: {status:?}");
}

#[test]
fn reload_with_invalid_json_keeps_active_graph() {
	// Compile failure during reload must not perturb the active graph;
	// in-flight + new connections continue to see the old rules.
	let tmp = tempfile::tempdir().expect("tempdir");
	let port = ephemeral_port();
	write_rule(tmp.path(), "site.json", &static_site_rule(port, "v1"));

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(port, Duration::from_secs(10));
	assert!(http_get(port).expect("v1 request").contains("v1"));

	// Corrupt the rule file. reload_once must fail at JSON parse and
	// leave the active graph untouched.
	fs::write(tmp.path().join("rules").join("site.json"), "{ this is not json").unwrap();
	std::thread::sleep(Duration::from_millis(700)); // 250ms debounce + buffer

	// "v1" must still serve.
	let resp = http_get(port).expect("post-bad-edit request");
	assert!(resp.contains("v1"), "active graph drifted after bad edit; got {resp:?}");

	kill_signal(&child, Signal::SIGTERM);
	let status = wait_with_timeout(&mut child, Duration::from_secs(5));
	assert!(status.success(), "SIGTERM exit: {status:?}");
}
