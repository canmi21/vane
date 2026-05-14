//! End-to-end test: `reverse_proxy` preset with `websocket: true`
//! actually proxies a WS handshake + byte tunnel through vaned.
//!
//! Live regression guard for the chain:
//! - `reverse_proxy` preset emits a `<name>.ws` rule with
//!   `match: http.header.upgrade == "websocket"` →
//!   `terminate: { type: "websocket", upstream: ... }` plus a
//!   `<name>.main` `http_proxy` rule.
//! - lower-and-link wires the WS rule into a `WebSocketUpgrade` fetch.
//! - the executor sends the request through that fetch on a real WS
//!   handshake and lets `drive_h1_server` bridge client ↔ upstream.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
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
		// Shrink the soft-drain window. Since J11 the H1 connection's
		// listener-side task stays alive for the full WS-tunnel
		// lifetime — that's the correct production behaviour, but
		// the test's 5-second `wait_with_timeout` budget doesn't have
		// room for the default 30 s drain. The test holds an active
		// WS connection at SIGTERM time; force_cancel firing at the
		// shorter drain boundary breaks the select inside the tunnel
		// task and lets the daemon exit promptly.
		.env("VANE_DRAIN_TIMEOUT_SECS", "1")
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

/// Read from `stream` until the buffered tail contains `\r\n\r\n`.
fn read_until_headers_end(stream: &mut TcpStream) -> Vec<u8> {
	let mut buf = Vec::new();
	let mut tmp = [0u8; 1024];
	loop {
		let n = stream.read(&mut tmp).expect("read");
		if n == 0 {
			break;
		}
		buf.extend_from_slice(&tmp[..n]);
		if buf.windows(4).any(|w| w == b"\r\n\r\n") {
			break;
		}
	}
	buf
}

/// Fake WS upstream: accept one connection, return 101, echo bytes.
/// Runs on a dedicated `std::thread` so it doesn't depend on the
/// test's tokio runtime.
struct FakeWsUpstream {
	port: u16,
	stop: Arc<AtomicBool>,
	join: Option<std::thread::JoinHandle<()>>,
}

impl FakeWsUpstream {
	fn spawn() -> Self {
		let listener = TcpListener::bind("127.0.0.1:0").expect("upstream bind");
		let port = listener.local_addr().expect("local addr").port();
		listener.set_nonblocking(true).expect("set nonblocking");
		let stop = Arc::new(AtomicBool::new(false));
		let stop_t = Arc::clone(&stop);
		let join = std::thread::spawn(move || {
			while !stop_t.load(Ordering::SeqCst) {
				match listener.accept() {
					Ok((mut stream, _peer)) => {
						stream.set_nonblocking(false).ok();
						stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
						stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
						let _ = read_until_headers_end_blocking(&mut stream);
						let _ = stream.write_all(
							b"HTTP/1.1 101 Switching Protocols\r\n\
							  Upgrade: websocket\r\n\
							  Connection: Upgrade\r\n\
							  Sec-WebSocket-Accept: RXEW6ax6BNRmDSUkBxiKlPFAoUM=\r\n\
							  \r\n",
						);
						// Echo loop until peer closes or the stop flag is set.
						let mut buf = [0u8; 4096];
						loop {
							if stop_t.load(Ordering::SeqCst) {
								break;
							}
							let n = match stream.read(&mut buf) {
								Ok(0) | Err(_) => break,
								Ok(n) => n,
							};
							if stream.write_all(&buf[..n]).is_err() {
								break;
							}
						}
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

fn read_until_headers_end_blocking(stream: &mut TcpStream) -> Vec<u8> {
	let mut buf = Vec::new();
	let mut tmp = [0u8; 1024];
	loop {
		let n = match stream.read(&mut tmp) {
			Ok(0) | Err(_) => break,
			Ok(n) => n,
		};
		buf.extend_from_slice(&tmp[..n]);
		if buf.windows(4).any(|w| w == b"\r\n\r\n") {
			break;
		}
	}
	buf
}

#[test]
fn reverse_proxy_websocket_true_passes_through_upgrade() {
	let upstream = FakeWsUpstream::spawn();
	let listen_port = ephemeral_port();
	let tmp = tempfile::tempdir().expect("tempdir");
	write_rule(
		tmp.path(),
		"ws.json",
		&format!(
			r#"{{
				"rules": [{{
					"preset": "reverse_proxy",
					"name": "ws",
					"listen": ["127.0.0.1:{listen_port}"],
					"args": {{
						"upstream": "127.0.0.1:{upstream_port}",
						"websocket": true,
						"forward_client_ip": false
					}}
				}}]
			}}"#,
			upstream_port = upstream.port,
		),
	);

	let mut child = spawn_vaned(tmp.path());
	wait_for_port_open(listen_port, Duration::from_secs(10));

	// Raw TCP client sends a WS upgrade request; vane proxies to the
	// upstream which always replies 101.
	let mut client = TcpStream::connect(format!("127.0.0.1:{listen_port}")).expect("client connect");
	client.set_read_timeout(Some(Duration::from_secs(3))).ok();
	client.set_write_timeout(Some(Duration::from_secs(3))).ok();
	let req = b"GET / HTTP/1.1\r\n\
		Host: example\r\n\
		Upgrade: websocket\r\n\
		Connection: Upgrade\r\n\
		Sec-WebSocket-Key: dGVzdGtleQ==\r\n\
		Sec-WebSocket-Version: 13\r\n\
		\r\n";
	client.write_all(req).expect("client write req");

	let head = read_until_headers_end(&mut client);
	let s = std::str::from_utf8(&head).expect("ascii head");
	assert!(s.starts_with("HTTP/1.1 101"), "expected 101 from preset path, got: {s}");
	assert!(
		s.to_lowercase().contains("upgrade: websocket"),
		"upstream upgrade headers should round-trip: {s}",
	);

	// Post-101 byte tunnel: write 5 bytes, expect them echoed back.
	client.write_all(b"hello").expect("write payload");
	let mut got = [0u8; 5];
	client.read_exact(&mut got).expect("read echo");
	assert_eq!(&got, b"hello");

	kill_signal(&child, Signal::SIGTERM);
	let status = wait_with_timeout(&mut child, Duration::from_secs(5));
	assert!(status.success(), "SIGTERM exit: {status:?}");
	upstream.stop();
}
