//! End-to-end tests for the daemon's WASM boot path: scan + lazy
//! runtime + plugin registry plumbing + boot ref-check.
//!
//! Each test spawns a real `vaned` subprocess against a tempdir
//! config tree. The fixture component lives in
//! `crates/wasm/fixtures/metadata_fixture.wasm` (built by
//! `crates/wasm/build.rs`); we copy it under a known stem so the
//! plugin reference name (`<stem>:probe`) is deterministic.

#![cfg(feature = "wasm")]

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use predicates::str::contains;
use vane_mgmt::HttpMgmtClient;
use vane_mgmt::verb::{GetPoolsResult, NoArgs, PingResult, VERB_GET_POOLS, VERB_PING};

fn ephemeral_port() -> u16 {
	let l = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
	let port = l.local_addr().expect("local addr").port();
	drop(l);
	port
}

fn fixture_wasm_path() -> &'static Path {
	Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../wasm/fixtures/metadata_fixture.wasm"))
}

/// Write a rule on `traffic_port`. With `mw_use = None` the rule
/// uses the `static_site` preset (no `middleware_chain`); with
/// `Some(name)` the rule is a raw rule whose `middleware_chain`
/// references the named middleware (native or plugin) — preset
/// rules drop unknown top-level keys, so the plugin reference has
/// to live on a raw rule for the boot ref-check to see it.
fn write_rule(dir: &Path, traffic_port: u16, mw_use: Option<&str>) {
	let rules_dir = dir.join("rules");
	std::fs::create_dir_all(&rules_dir).expect("rules/");
	let body = match mw_use {
		None => format!(
			r#"{{
				"rules": [{{
					"preset": "static_site",
					"name": "site",
					"listen": ["127.0.0.1:{traffic_port}"],
					"args": {{ "status": 200, "body": "ok" }}
				}}]
			}}"#
		),
		Some(name) => format!(
			// L4 forward terminate keeps the rule's phase
			// constraint at L4 — compatible with the metadata
			// fixture's `probe` export (`L4Peek`). Using an L7
			// terminate (e.g. `static`) would phase-mismatch.
			r#"{{
				"rules": [{{
					"name": "site",
					"listen": ["127.0.0.1:{traffic_port}"],
					"middleware_chain": [{{ "use": "{name}" }}],
					"terminate": {{
						"type": "tcp_forward",
						"upstream": "127.0.0.1:1"
					}}
				}}]
			}}"#
		),
	};
	std::fs::write(rules_dir.join("site.json"), body).expect("write rule");
}

/// Copy the metadata fixture into `wasm_dir` under `stem`. Returns
/// the destination path so tests can assert against it.
fn install_fixture_as(wasm_dir: &Path, stem: &str) {
	std::fs::create_dir_all(wasm_dir).expect("wasm dir");
	std::fs::copy(fixture_wasm_path(), wasm_dir.join(format!("{stem}.wasm")))
		.expect("copy fixture into wasm_dir");
}

struct Daemon {
	child: std::process::Child,
	mgmt_addr: SocketAddr,
	_tmp: tempfile::TempDir,
}

impl Drop for Daemon {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

/// Spawn a daemon with HTTP mgmt + bearer token, the supplied rule,
/// and an explicit `wasm_dir`. Wait until the mgmt port accepts TCP.
fn spawn_daemon(config_dir: tempfile::TempDir, mgmt_port: u16, token: &str) -> Daemon {
	let mut cmd = std::process::Command::cargo_bin("vaned").expect("locate vaned");
	cmd
		.arg("-c")
		.arg(config_dir.path())
		.env("VANE_MGMT_UNIX", config_dir.path().join("vaned.sock"))
		.env("VANE_MGMT_HTTP_PORT", mgmt_port.to_string())
		.env("VANE_MGMT_HTTP_TOKEN", token)
		.env("VANE_WASM_DIR", config_dir.path().join("wasm"))
		.env("RUST_LOG", "warn")
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	let child = cmd.spawn().expect("spawn vaned");
	let mgmt_addr: SocketAddr = format!("127.0.0.1:{mgmt_port}").parse().expect("addr");
	wait_for_listener(mgmt_addr, Duration::from_secs(5));
	Daemon { child, mgmt_addr, _tmp: config_dir }
}

fn wait_for_listener(addr: SocketAddr, timeout: Duration) {
	let deadline = Instant::now() + timeout;
	while Instant::now() < deadline {
		if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
			return;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	panic!("mgmt http listener {addr} did not bind within {timeout:?}");
}

// ─── boot path tests ──────────────────────────────────────────────────

#[tokio::test]
async fn daemon_starts_when_wasm_dir_is_absent() {
	// VANE_WASM_DIR points at a non-existent path → loader returns
	// None → daemon falls through to the no-plugin link path. Should
	// boot cleanly and serve mgmt; get_pools.wasm stays empty.
	let tmp = tempfile::tempdir().expect("tempdir");
	let traffic_port = ephemeral_port();
	let mgmt_port = ephemeral_port();
	write_rule(tmp.path(), traffic_port, None);
	// Deliberately do NOT create `wasm/` under tmp.
	let d = spawn_daemon(tmp, mgmt_port, "abs-tok");
	let client = HttpMgmtClient::new(d.mgmt_addr, Some(Arc::<str>::from("abs-tok")));
	let r: PingResult = client.call(VERB_PING, &NoArgs {}).await.expect("ping");
	assert!(r.pong);
	let p: GetPoolsResult = client.call(VERB_GET_POOLS, &NoArgs {}).await.expect("get_pools");
	assert!(p.wasm.is_empty(), "no wasm dir → no registered plugins");
}

#[tokio::test]
async fn daemon_starts_with_wasm_modules_loaded_but_no_rules_referencing_them() {
	// wasm_dir contains a valid fixture, but no rule uses it. Daemon
	// boots, loader registers exports, ref-check passes (no rule
	// references a plugin), runtime is alive but its pool snapshot
	// is empty until a rule actually links a stateful pool.
	let tmp = tempfile::tempdir().expect("tempdir");
	let traffic_port = ephemeral_port();
	let mgmt_port = ephemeral_port();
	write_rule(tmp.path(), traffic_port, None);
	install_fixture_as(&tmp.path().join("wasm"), "plugin_a");
	let d = spawn_daemon(tmp, mgmt_port, "loaded-tok");
	let client = HttpMgmtClient::new(d.mgmt_addr, Some(Arc::<str>::from("loaded-tok")));
	let r: PingResult = client.call(VERB_PING, &NoArgs {}).await.expect("ping");
	assert!(r.pong);
	let p: GetPoolsResult = client.call(VERB_GET_POOLS, &NoArgs {}).await.expect("get_pools");
	// L4Peek probe is stateless on the fixture; no rule references it
	// so no pool was instantiated. The wasm_pool_stats snapshot is
	// expected to be empty.
	assert!(p.wasm.is_empty(), "runtime alive but no rule references plugin → wasm pool list empty");
}

#[tokio::test]
async fn daemon_starts_with_rule_referencing_loaded_plugin() {
	// Rule references `<stem>:probe` (the fixture's L4Peek export).
	// The boot ref-check must accept it (registry has the entry),
	// link must succeed, and the daemon must come up.
	let tmp = tempfile::tempdir().expect("tempdir");
	let traffic_port = ephemeral_port();
	let mgmt_port = ephemeral_port();
	install_fixture_as(&tmp.path().join("wasm"), "edge");
	write_rule(tmp.path(), traffic_port, Some("edge:probe"));
	let d = spawn_daemon(tmp, mgmt_port, "edge-tok");
	let client = HttpMgmtClient::new(d.mgmt_addr, Some(Arc::<str>::from("edge-tok")));
	let r: PingResult = client.call(VERB_PING, &NoArgs {}).await.expect("ping");
	assert!(r.pong);
}

// ─── boot refusal test ────────────────────────────────────────────────

#[test]
fn daemon_refuses_to_start_when_rule_references_unloaded_plugin() {
	// Rule uses `missing:probe`, but the wasm dir is empty. Boot
	// ref-check must fail with a message that names the missing
	// plugin so the operator can fix it without grepping logs.
	let tmp = tempfile::tempdir().expect("tempdir");
	let traffic_port = ephemeral_port();
	let mgmt_port = ephemeral_port();
	write_rule(tmp.path(), traffic_port, Some("missing:probe"));
	std::fs::create_dir_all(tmp.path().join("wasm")).expect("empty wasm dir");

	assert_cmd::Command::cargo_bin("vaned")
		.expect("bin")
		.arg("-c")
		.arg(tmp.path())
		.env("VANE_MGMT_UNIX", tmp.path().join("vaned.sock"))
		.env("VANE_MGMT_HTTP_PORT", mgmt_port.to_string())
		.env("VANE_WASM_DIR", tmp.path().join("wasm"))
		.env("RUST_LOG", "warn")
		.assert()
		.failure()
		.stderr(contains("missing:probe"))
		.stderr(contains("refusing to start"));
	// `traffic_port` reserved above is intentionally unused — the
	// daemon never reaches listener bind on the refusal path.
	let _ = traffic_port;
}
