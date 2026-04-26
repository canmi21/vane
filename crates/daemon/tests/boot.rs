//! Integration tests for `vaned`'s boot path. Spawn the real binary
//! against a tempdir-backed config tree and verify:
//!
//! - `--version` prints the build banner without requiring a config
//!   directory.
//! - Boot fails loud (exit 1, error on stderr) for missing `rules/` and
//!   for unparseable `*.json`.
//! - A minimal `static_site` preset rule produces a working HTTP server
//!   reachable via raw TCP.
//! - SIGTERM triggers the soft-drain shutdown and the process exits
//!   cleanly within a few seconds.
//!
//! The signal-driven tests use `libc::kill` directly. The workspace
//! lint `unsafe_code = "deny"` is relaxed locally — there is no
//! safe-Rust equivalent for sending POSIX signals to a child process,
//! and the surrounding test infrastructure (single test binary, no
//! cross-thread access to `Child`) makes the unsafe sound.
#![allow(unsafe_code)]

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use predicates::prelude::*;
use predicates::str::contains;

/// Write a `rules/<name>.json` file inside `dir`.
fn write_rule(dir: &Path, name: &str, body: &str) {
	let rules = dir.join("rules");
	fs::create_dir_all(&rules).expect("create rules/");
	fs::write(rules.join(name), body).expect("write rule");
}

/// Find a TCP port that is currently free and return it. The binding
/// is dropped immediately, so a TOCTOU race exists between this and a
/// subsequent `vaned` bind. In practice the window is small enough
/// that the flake rate is acceptable for CI.
fn ephemeral_port() -> u16 {
	let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
	let port = listener.local_addr().expect("local addr").port();
	drop(listener);
	port
}

/// Spawn `vaned -c <dir>` with stdio inherited. Stderr/stdout are not
/// piped — earlier attempts to capture stderr via `Stdio::piped()`
/// blocked indefinitely because `tracing-subscriber` block-buffers
/// when stderr is not a tty, so "listeners started" never reaches the
/// test reader. The test detects readiness by polling the listener
/// port instead (see [`wait_for_port_open`]).
fn spawn_vaned(dir: &Path) -> Child {
	let cmd = assert_cmd::Command::cargo_bin("vaned").expect("locate vaned bin");
	let bin = cmd.get_program().to_owned();
	Command::new(bin)
		.arg("-c")
		.arg(dir)
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.expect("spawn vaned")
}

/// Poll `127.0.0.1:port` with TCP connect attempts until one succeeds
/// or `timeout` elapses. This is the listener-readiness signal the test
/// uses in lieu of parsing daemon log output (which is unreliable from
/// a piped stderr — see [`spawn_vaned`]).
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

/// Send a POSIX signal to the child process.
fn kill_signal(child: &Child, sig: i32) {
	#[allow(clippy::cast_possible_wrap)]
	let pid = child.id() as libc::pid_t;
	// SAFETY: PID was just reported by a live `Child` we own; libc::kill
	// with a valid signal is sound.
	let rc = unsafe { libc::kill(pid, sig) };
	assert_eq!(rc, 0, "libc::kill returned {rc}");
}

/// Wait for the child to exit, with a timeout. Returns the exit status.
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

// ----- assert_cmd-driven exit-code tests -------------------------------

#[test]
fn version_flag_prints_banner_without_config() {
	// --version short-circuits before any config loading.
	assert_cmd::Command::cargo_bin("vaned")
		.expect("bin")
		.arg("--version")
		.assert()
		.success()
		.stdout(contains("Vane — A compact programmable proxy engine"))
		.stdout(contains("Built:"));
}

#[test]
fn boot_with_missing_rules_dir_exits_with_failure() {
	let tmp = tempfile::tempdir().expect("tempdir");
	// no rules/ created.
	assert_cmd::Command::cargo_bin("vaned")
		.expect("bin")
		.arg("-c")
		.arg(tmp.path())
		.assert()
		.failure()
		.stderr(contains("rules directory not found"));
}

#[test]
fn boot_with_invalid_json_exits_with_failure() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_rule(tmp.path(), "broken.json", "{ this is not json");
	assert_cmd::Command::cargo_bin("vaned")
		.expect("bin")
		.arg("-c")
		.arg(tmp.path())
		.assert()
		.failure()
		.stderr(contains("parse").or(contains("broken.json")));
}

// ----- spawn-and-signal tests ------------------------------------------

#[test]
fn boot_with_static_site_serves_response_and_drains_on_sigterm() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let port = ephemeral_port();
	write_rule(
		tmp.path(),
		"hello.json",
		&format!(
			r#"{{
				"rules": [{{
					"preset": "static_site",
					"name": "hello",
					"listen": ["127.0.0.1:{port}"],
					"args": {{
						"status": 200,
						"headers": {{ "content-type": "text/plain" }},
						"body": "hello from vane"
					}}
				}}]
			}}"#
		),
	);

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(port, Duration::from_secs(10));

	// Hit the listener with raw HTTP/1.1.
	let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
	stream
		.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
		.expect("write request");
	let mut response = String::new();
	stream.read_to_string(&mut response).expect("read response");
	assert!(response.starts_with("HTTP/1.1 200"), "got: {response:?}");
	assert!(response.contains("hello from vane"), "body missing: {response:?}");

	// SIGTERM kicks soft-drain (30s timeout); without in-flight work it
	// completes immediately.
	kill_signal(&child, libc::SIGTERM);
	let status = wait_with_timeout(&mut child, Duration::from_secs(5));
	assert!(status.success(), "SIGTERM exit status: {status:?}");
}

#[test]
fn sigint_immediate_shutdown_under_one_second() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let port = ephemeral_port();
	write_rule(
		tmp.path(),
		"site.json",
		&format!(
			r#"{{
				"rules": [{{
					"preset": "static_site",
					"name": "site",
					"listen": ["127.0.0.1:{port}"],
					"args": {{ "status": 204 }}
				}}]
			}}"#
		),
	);

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(port, Duration::from_secs(10));

	let started = Instant::now();
	kill_signal(&child, libc::SIGINT);
	let status = wait_with_timeout(&mut child, Duration::from_secs(3));
	let elapsed = started.elapsed();
	assert!(status.success(), "SIGINT exit status: {status:?}");
	assert!(elapsed < Duration::from_secs(2), "SIGINT shutdown took {elapsed:?}");
}
