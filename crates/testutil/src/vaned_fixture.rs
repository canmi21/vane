//! `VanedFixture` — owns a tmp config dir + Unix-socket path + spawned
//! `vaned` child process, waits for socket-ready, auto-teardowns on
//! `Drop`. Drives end-to-end tests.
//!
//! The fixture authors config by invoking the **real `vane` CLI** and
//! serves it with the **real `vaned`** binary, so a scenario exercises
//! the exact path an operator walks (scaffold → author → start), not an
//! in-test synthetic config. Binaries are resolved via the `VANE_BIN` /
//! `VANED_BIN` env vars (exported by the nextest `build-test-bins` setup
//! script) with an escargot build as the `cargo test` fallback.
//!
//! See [`spec/conventions.md` § _Test surface by binary kind_](../../../spec/conventions.md#test-surface-by-binary-kind).

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Resolve a workspace binary: env-var fast path first (set by the
/// nextest `build-test-bins` setup script), else build via escargot.
fn resolve_bin(pkg: &str, env_var: &str) -> PathBuf {
	if let Some(p) = std::env::var_os(env_var) {
		return PathBuf::from(p);
	}
	escargot::CargoBuild::new()
		.package(pkg)
		.bin(pkg)
		.current_release()
		.current_target()
		.run()
		.unwrap_or_else(|e| panic!("escargot build of `{pkg}` failed: {e}"))
		.path()
		.to_path_buf()
}

fn vane_bin() -> &'static Path {
	static BIN: OnceLock<PathBuf> = OnceLock::new();
	BIN.get_or_init(|| resolve_bin("vane", "VANE_BIN"))
}

fn vaned_bin() -> &'static Path {
	static BIN: OnceLock<PathBuf> = OnceLock::new();
	BIN.get_or_init(|| resolve_bin("vaned", "VANED_BIN"))
}

/// A temp config tree authored through the real `vane` CLI, ready to be
/// [`start`](VanedFixture::start)ed into a running `vaned`.
pub struct VanedFixture {
	tmp: TempDir,
	config_dir: PathBuf,
}

impl VanedFixture {
	/// Create a fresh temp config tree, scaffolded by the real `vane init`.
	#[must_use]
	pub fn new() -> Self {
		let tmp = tempfile::tempdir().expect("create temp config dir");
		let config_dir = tmp.path().to_path_buf();
		let f = Self { tmp, config_dir };
		f.run_vane(&["init", f.dir_str()]);
		f
	}

	/// The config directory `vaned` will be pointed at.
	pub fn config_dir(&self) -> &Path {
		&self.config_dir
	}

	fn dir_str(&self) -> &str {
		self.config_dir.to_str().expect("config dir is valid UTF-8")
	}

	/// Run the real `vane` CLI with `args`; panic with stderr on failure.
	pub fn run_vane(&self, args: &[&str]) -> std::process::Output {
		let out = Command::new(vane_bin()).args(args).output().expect("spawn vane CLI");
		assert!(
			out.status.success(),
			"`vane {}` failed: {}",
			args.join(" "),
			String::from_utf8_lossy(&out.stderr)
		);
		out
	}

	/// Author an L4 port-forward rule via `vane add port-forward`.
	pub fn add_port_forward(&self, name: &str, listen: &str, to: &str) {
		self.run_vane(&[
			"add",
			"port-forward",
			"--dir",
			self.dir_str(),
			"--name",
			name,
			"--listen",
			listen,
			"--to",
			to,
		]);
	}

	/// Author an HTTP reverse-proxy rule via `vane add reverse-proxy`.
	pub fn add_reverse_proxy(&self, name: &str, listen: &str, to: &str) {
		self.run_vane(&[
			"add",
			"reverse-proxy",
			"--dir",
			self.dir_str(),
			"--name",
			name,
			"--listen",
			listen,
			"--to",
			to,
		]);
	}

	/// Author a fixed-response rule via `vane add static-site`.
	pub fn add_static_site(&self, name: &str, listen: &str, status: u16, body: &str) {
		let status = status.to_string();
		self.run_vane(&[
			"add",
			"static-site",
			"--dir",
			self.dir_str(),
			"--name",
			name,
			"--listen",
			listen,
			"--status",
			&status,
			"--body",
			body,
		]);
	}

	/// Spawn `vaned -c <dir>` with an isolated mgmt socket and the HTTP
	/// mgmt transport disabled (so parallel daemons don't fight over the
	/// fixed mgmt port). Returns a guard that SIGKILL-tears-down on Drop.
	#[must_use]
	pub fn start(self) -> Vaned {
		let socket = self.config_dir.join("vaned.sock");
		let child = Command::new(vaned_bin())
			.arg("-c")
			.arg(&self.config_dir)
			.env("VANE_MGMT_UNIX", &socket)
			.env("VANE_MGMT_HTTP_PORT", "")
			// Redirect all daemon state under the tempdir so a non-root
			// test run never touches (or warns about) /var/lib/vaned.
			.env("VANE_STATE_DIR", self.config_dir.join("state"))
			.env("RUST_LOG", "warn")
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.spawn()
			.expect("spawn vaned");
		Vaned { child, _tmp: self.tmp, config_dir: self.config_dir, socket }
	}
}

impl Default for VanedFixture {
	fn default() -> Self {
		Self::new()
	}
}

/// A running `vaned` child process. SIGKILL-tears-down on `Drop`.
pub struct Vaned {
	child: Child,
	_tmp: TempDir,
	config_dir: PathBuf,
	socket: PathBuf,
}

impl Vaned {
	/// The mgmt Unix socket path (feed to `vane_mgmt::UnixMgmtClient`).
	pub fn socket(&self) -> &Path {
		&self.socket
	}

	/// The config directory this daemon was started with.
	pub fn config_dir(&self) -> &Path {
		&self.config_dir
	}

	/// Block until `addr` accepts a TCP connection, or panic after
	/// `timeout`. Boot is not "ready" at spawn — the listener binds a
	/// beat after the process starts.
	pub fn wait_listener(&self, addr: SocketAddr, timeout: Duration) {
		let deadline = Instant::now() + timeout;
		while Instant::now() < deadline {
			if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
				return;
			}
			std::thread::sleep(Duration::from_millis(50));
		}
		panic!("vaned listener {addr} did not bind within {timeout:?}");
	}
}

impl Drop for Vaned {
	fn drop(&mut self) {
		// Mirror the proven teardown in daemon mgmt tests: skip the
		// SIGKILL path if the child already exited, else kill + bounded
		// wait so the kernel releases the listener socket before a
		// sibling test recycles the port.
		if matches!(self.child.try_wait(), Ok(Some(_))) {
			return;
		}
		let _ = self.child.kill();
		for _ in 0..50 {
			match self.child.try_wait() {
				Ok(Some(_)) | Err(_) => return,
				Ok(None) => std::thread::sleep(Duration::from_millis(20)),
			}
		}
	}
}

/// Minimal blocking HTTP/1.1 GET against `addr`. Sends `Connection:
/// close` so the server closes after responding (clean EOF); returns the
/// full raw response (status line + headers + body) as lossy UTF-8.
pub fn http_get(addr: SocketAddr, path: &str) -> String {
	let mut stream = TcpStream::connect(addr).expect("connect to vaned listener");
	let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
	let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
	write!(stream, "GET {path} HTTP/1.1\r\nHost: vane-test\r\nConnection: close\r\n\r\n")
		.expect("write HTTP request");
	let mut buf = Vec::new();
	let _ = stream.read_to_end(&mut buf);
	String::from_utf8_lossy(&buf).into_owned()
}

/// Generate a self-signed cert + key for `host`, write them as PEM into
/// `dir` (`cert.pem` / `key.pem`), and return their paths — for authoring
/// a static TLS-terminating rule in a scenario.
pub fn gen_self_signed(dir: &Path, host: &str) -> (PathBuf, PathBuf) {
	let issued =
		rcgen::generate_simple_self_signed(vec![host.to_owned()]).expect("generate self-signed cert");
	let cert_path = dir.join("cert.pem");
	let key_path = dir.join("key.pem");
	std::fs::write(&cert_path, issued.cert.pem()).expect("write cert.pem");
	std::fs::write(&key_path, issued.signing_key.serialize_pem()).expect("write key.pem");
	(cert_path, key_path)
}

/// Blocking HTTPS GET via `curl -sk` (accepts the self-signed test cert).
/// Returns `(status, body)`. Requires `curl` on PATH — present on dev/CI;
/// keeps the harness's subprocess shape rather than embedding a TLS client.
pub fn https_get(port: u16) -> (u16, String) {
	let url = format!("https://127.0.0.1:{port}/");
	// Negotiate HTTP/2 over TLS (vane offers h2 via ALPN) — the realistic
	// browser path, exercising the H2-in -> upstream bridge, not just TLS
	// framing.
	let out = std::process::Command::new("curl")
		.args(["-sk", "--http2", "-o", "-", "-w", "\n__VANE_STATUS__%{http_code}", &url])
		.output()
		.expect("run curl (required for HTTPS scenarios)");
	let s = String::from_utf8_lossy(&out.stdout).into_owned();
	match s.rsplit_once("__VANE_STATUS__") {
		Some((body, code)) => (code.trim().parse().unwrap_or(0), body.to_owned()),
		None => (0, s),
	}
}
