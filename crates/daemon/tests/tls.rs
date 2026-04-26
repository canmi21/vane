//! End-to-end coverage for upstream TLS in the running daemon.
//!
//! Spins up a self-signed `tokio_rustls` HTTPS service on an
//! ephemeral port, writes a raw `http_proxy` rule with
//! `terminate.tls.insecure_skip_verify: true` pointing at it, boots
//! `vaned` against that config, and verifies a cleartext client GET
//! against vane's listener round-trips through to the HTTPS upstream
//! and back. Mirrors the dispatching shape used by `tests/mgmt.rs`
//! but exercises the data plane rather than the mgmt plane.

#![allow(clippy::too_many_lines)]

use std::io::{Read, Write};
use std::net::TcpListener as StdTcpListener;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

struct Daemon {
	child: std::process::Child,
	_tmp: tempfile::TempDir,
}

impl Drop for Daemon {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

/// rcgen self-signed cert for `localhost`, packaged into a
/// `rustls::ServerConfig` that hyper's HTTP/1.1 server can use.
fn rcgen_server_config() -> Arc<rustls::ServerConfig> {
	let issued =
		rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("self-signed cert");
	let cert_der: CertificateDer<'static> = issued.cert.der().clone();
	let key_der: PrivateKeyDer<'static> =
		PrivateKeyDer::Pkcs8(issued.signing_key.serialize_der().into());
	let cfg = rustls::ServerConfig::builder()
		.with_no_client_auth()
		.with_single_cert(vec![cert_der], key_der)
		.expect("build server config");
	Arc::new(cfg)
}

/// Start a one-shot HTTPS server: accepts a single TLS connection,
/// answers any request with `body`, returns the assigned address.
async fn spawn_https_static(
	body: &'static str,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
	vane_engine::crypto::install_default_provider();
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind https");
	let addr = listener.local_addr().expect("local_addr");
	let server_config = rcgen_server_config();
	let acceptor = TlsAcceptor::from(server_config);
	let handle = tokio::spawn(async move {
		// Accept at least one connection; loop in case hyper's H1
		// client opens a fresh connection per request (which is the
		// daemon-side behaviour today since the upstream pool was
		// dropped when TLS landed).
		loop {
			let Ok((sock, _)) = listener.accept().await else { return };
			let acceptor = acceptor.clone();
			tokio::spawn(async move {
				let Ok(tls) = acceptor.accept(sock).await else { return };
				let io = TokioIo::new(tls);
				let svc = hyper::service::service_fn(
					move |_req: hyper::Request<hyper::body::Incoming>| async move {
						Ok::<_, std::convert::Infallible>(
							hyper::Response::builder()
								.status(200)
								.header("content-type", "text/plain")
								.body(Full::new(Bytes::from(body.to_string())))
								.expect("build response"),
						)
					},
				);
				let _ = hyper::server::conn::http1::Builder::new().serve_connection(io, svc).await;
			});
		}
	});
	(addr, handle)
}

/// Choose a port that's almost certainly free on `127.0.0.1`. Bind
/// briefly to grab the OS-assigned port, then drop. The vaned
/// listener race window is small in practice (no other test binds
/// the same range immediately after) and sufficient for the test.
fn pick_free_port() -> u16 {
	let l = StdTcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
	let port = l.local_addr().expect("local_addr").port();
	drop(l);
	port
}

/// Write a single rule under `dir/rules/proxy.json` that terminates
/// matched traffic into an `http_proxy` fetch with TLS to
/// `upstream`. Uses the raw rule shape (not a preset) because the
/// `reverse_proxy` preset doesn't yet expose `upstream_tls` — this
/// is a deferred preset surface, flagged in the engine commit.
fn write_https_proxy_rule(dir: &Path, listen_port: u16, upstream: &str) {
	let rules = dir.join("rules");
	std::fs::create_dir_all(&rules).expect("rules/");
	std::fs::write(
		rules.join("proxy.json"),
		format!(
			r#"{{
				"rules": [{{
					"name": "https-up",
					"listen": ["127.0.0.1:{listen_port}"],
					"terminate": {{
						"type": "http_proxy",
						"upstream": "{upstream}",
						"tls": {{
							"insecure_skip_verify": true,
							"verify_hostname": "localhost"
						}}
					}}
				}}]
			}}"#
		),
	)
	.expect("write rule");
}

fn spawn_vaned(config_dir: &Path, socket: &Path) -> Daemon {
	let mut cmd = std::process::Command::cargo_bin("vaned").expect("locate vaned");
	cmd
		.arg("-c")
		.arg(config_dir)
		.env("VANE_MGMT_UNIX", socket)
		.env("RUST_LOG", "warn")
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	let child = cmd.spawn().expect("spawn vaned");
	Daemon { child, _tmp: tempfile::tempdir().expect("placeholder") }
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

fn wait_for_tcp_listener(addr: std::net::SocketAddr, timeout: Duration) {
	let deadline = Instant::now() + timeout;
	while Instant::now() < deadline {
		if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
			return;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	panic!("listener {addr} did not bind within {timeout:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vaned_proxies_cleartext_client_to_https_upstream() {
	// Stand up the HTTPS upstream first so we know its port before
	// writing the rule that points at it.
	let (upstream_addr, _upstream_task) = spawn_https_static("hello-from-https").await;
	let upstream = upstream_addr.to_string();
	let listen_port = pick_free_port();

	let config_dir_keep = tempfile::tempdir().expect("config tempdir");
	let socket_dir_keep = tempfile::tempdir().expect("socket tempdir");
	write_https_proxy_rule(config_dir_keep.path(), listen_port, &upstream);
	let socket = socket_dir_keep.path().join("vaned.sock");

	let daemon = spawn_vaned(config_dir_keep.path(), &socket);
	wait_for_socket(&socket, Duration::from_secs(5));
	let listen_addr: std::net::SocketAddr = format!("127.0.0.1:{listen_port}").parse().unwrap();
	wait_for_tcp_listener(listen_addr, Duration::from_secs(5));

	// Cleartext HTTP client → vane listener. Use a raw TcpStream + a
	// minimal HTTP/1.1 request so we don't depend on a client library
	// — the request format is well-defined.
	let mut stream = std::net::TcpStream::connect(listen_addr).expect("connect");
	stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
	stream
		.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
		.expect("write request");
	stream.flush().ok();
	// Don't half-close the write side — hyper's H1 server treats early
	// EOF as a `serve_connection` protocol error and tears down the
	// keep-alive loop before the upstream response arrives.
	let mut buf = Vec::new();
	stream.read_to_end(&mut buf).expect("read response");

	let response = String::from_utf8_lossy(&buf);
	assert!(response.starts_with("HTTP/1.1 200"), "expected 200 status, got: {response}");
	assert!(
		response.contains("hello-from-https"),
		"expected upstream body in response, got: {response}",
	);

	// Drop the daemon explicitly so the upstream task winds down
	// before the temp dirs go away.
	drop(daemon);
	drop(socket_dir_keep);
	drop(config_dir_keep);
}
