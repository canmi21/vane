//! End-to-end test: `rate_limit` middleware through the
//! `reverse_proxy` preset surfaces a real 429 on the wire after the
//! bucket is exhausted.
//!
//! This is the live regression guard for the chain of changes that
//! makes `Short(Response)` actually work:
//! - lower synthesises a `Terminate(WriteHttpResponse)` per L7
//!   listener and stores it in `meta.short_circuit_response_entry`,
//! - the executor's `Short(Response)` arm jumps to that synth target
//!   instead of stubbing an internal error,
//! - `rate_limit` emits `Short(Response(429))` on bucket exhaustion.
//!
//! Without any one of those, this test fails with a 500 (executor
//! error) or a 404 (route mishandling).

use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// One-shot HTTP/1.1 client. Returns the full response (status line +
/// headers + body). Caller asserts on substrings.
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

/// Tiny fake upstream: accepts every connection, writes a fixed
/// HTTP/1.1 200 response, half-closes the write side, drops the
/// stream. Used as the `upstream` for vaned's `reverse_proxy` preset
/// fixture so the proxy hit returns 200 cleanly.
///
/// The thread loops until `stop.store(true)`; the test triggers the
/// stop after the daemon has exited so accept doesn't outlive the
/// child process.
struct FakeUpstream {
	port: u16,
	stop: Arc<AtomicBool>,
	join: Option<std::thread::JoinHandle<()>>,
}

impl FakeUpstream {
	fn spawn() -> Self {
		let listener = TcpListener::bind("127.0.0.1:0").expect("fake upstream bind");
		let port = listener.local_addr().expect("local addr").port();
		listener.set_nonblocking(true).expect("set fake upstream nonblocking");
		let stop = Arc::new(AtomicBool::new(false));
		let stop_t = Arc::clone(&stop);
		let join = std::thread::spawn(move || {
			while !stop_t.load(Ordering::SeqCst) {
				match listener.accept() {
					Ok((mut stream, _addr)) => {
						let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
						let _ = stream
							.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
						let _ = stream.shutdown(Shutdown::Write);
					}
					Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
						std::thread::sleep(Duration::from_millis(20));
					}
					Err(_) => break,
				}
			}
		});
		Self { port, stop, join: Some(join) }
	}

	fn stop(mut self) {
		self.stop.store(true, Ordering::SeqCst);
		if let Some(h) = self.join.take() {
			let _ = h.join();
		}
	}
}

#[test]
fn rate_limit_429_observable_through_reverse_proxy_preset() {
	// Burst=2, rate=1/60s — first two requests hit upstream with 200,
	// third request finds the bucket empty and the rate_limit middleware
	// returns Short(Response(429)). The chain is:
	//
	//   reverse_proxy preset → emits a rate_limit middleware ref +
	//   http_proxy fetch on listen=:listen_port
	//   → lower synthesises Terminate(WriteHttpResponse) for the listener
	//     and populates meta.short_circuit_response_entry
	//   → executor on Short(Response) jumps to the synth target
	//   → service-fn yields the 429 to hyper.
	//
	// Without lower-synth + executor-routing, the third request would
	// fail with 500 (Error::internal stub) instead of 429.
	let upstream = FakeUpstream::spawn();
	let listen_port = ephemeral_port();
	let tmp = tempfile::tempdir().expect("tempdir");
	write_rule(
		tmp.path(),
		"limited.json",
		&format!(
			r#"{{
				"rules": [{{
					"preset": "reverse_proxy",
					"name": "limited",
					"listen": ["127.0.0.1:{listen_port}"],
					"args": {{
						"upstream": "127.0.0.1:{upstream_port}",
						"rate_limit": {{ "rate": 1, "burst": 2, "window": "60s" }}
					}}
				}}]
			}}"#,
			upstream_port = upstream.port,
		),
	);

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(listen_port, Duration::from_secs(10));

	// First two requests pass.
	let r1 = http_get(listen_port).expect("first request");
	assert!(r1.starts_with("HTTP/1.1 200"), "first request must hit upstream: {r1:?}");
	let r2 = http_get(listen_port).expect("second request");
	assert!(r2.starts_with("HTTP/1.1 200"), "second request must hit upstream: {r2:?}");

	// Third exhausts the bucket → rate_limit emits Short(Response(429)),
	// the executor routes through the synth Terminate(WriteHttpResponse),
	// and 429 hits the wire.
	let r3 = http_get(listen_port).expect("third request");
	assert!(
		r3.starts_with("HTTP/1.1 429"),
		"third request must surface 429 (not 500 or 404 — both signal a routing regression): {r3:?}",
	);

	kill_signal(&child, Signal::SIGTERM);
	let status = wait_with_timeout(&mut child, Duration::from_secs(5));
	assert!(status.success(), "SIGTERM exit: {status:?}");
	upstream.stop();
}
