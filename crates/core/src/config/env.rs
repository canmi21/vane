//! Typed accessors for `VANE_*` deployment-constant env vars
//! (`spec/crates/core.md` § _Config layers_).
//!
//! The [`EnvReader`] trait abstracts the source so unit tests pass a
//! `HashMap`-backed fake instead of mutating process-global state — Rust
//! 1.95 marks `std::env::set_var` `unsafe` due to multi-thread races.

use std::path::{Path, PathBuf};

use crate::error::Error;

/// Reads a key → optional string value. The single production
/// implementation, [`ProcessEnv`], delegates to `std::env::var`. Tests
/// hand-roll a fake `EnvReader` to keep state local.
pub trait EnvReader {
	fn get(&self, key: &str) -> Option<String>;
}

/// Production [`EnvReader`] — reads from `std::env`.
pub struct ProcessEnv;

impl EnvReader for ProcessEnv {
	fn get(&self, key: &str) -> Option<String> {
		std::env::var(key).ok()
	}
}

/// Typed snapshot of every `VANE_*` deployment constant the daemon
/// reads at startup. Defaults match `spec/crates/core.md`
/// § _Config layers_.
///
/// `config_dir` is **not** modeled as a field — the daemon's `--config`
/// CLI arg is the single source of truth, and [`Env::from_reader`]
/// takes that path explicitly so derived defaults (`wasm_dir`) follow
/// it without an extra env var to keep in sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
	/// `VANE_WASM_DIR` — WASM plugin source directory scanned at boot.
	/// Defaults to `<config_dir>/wasm` where `config_dir` is the
	/// daemon's `--config` argument. See
	/// `spec/crates/engine-wasm.md` § _Module lifecycle_.
	pub wasm_dir: PathBuf,
	/// `VANE_LOG_LEVEL` — `tracing-subscriber` filter directive
	/// (default `"info"`). Honors the same syntax as `RUST_LOG`
	/// (per-target overrides like `vane=debug,hyper=warn`). The
	/// process env `RUST_LOG`, when set, takes precedence so
	/// operators can override the file value ad-hoc.
	pub log_level: String,
	/// `VANE_BIND_IPV4` — listen on 0.0.0.0 for `:N` listen specs (default `true`).
	pub bind_ipv4: bool,
	/// `VANE_BIND_IPV6` — listen on `[::]` for `:N` listen specs (default `true`).
	pub bind_ipv6: bool,
	/// `VANE_SEC_MAX_HEADER_BYTES` — request-header size cap (default 65536).
	pub sec_max_header_bytes: u32,
	/// `VANE_SEC_MAX_HEADERS_COUNT` — request-header count cap (default 100).
	pub sec_max_headers_count: u32,
	/// `VANE_SEC_HEADER_TIMEOUT` — header-completion timeout, seconds (default 30).
	pub sec_header_timeout_secs: u32,
	/// `VANE_SEC_MAX_CONN_PER_IP` — per-IP concurrent-connection cap (default 100).
	pub sec_max_conn_per_ip: u32,
	/// `VANE_SEC_MAX_TOTAL_CONNS` — daemon-wide concurrent-connection cap (default 65536).
	pub sec_max_total_conns: u32,
	/// `VANE_BIND_MAX_ATTEMPTS` — bind-retry count per listener address (default 10).
	pub bind_max_attempts: u32,
	/// `VANE_BIND_BACKOFF_INITIAL_MS` — initial retry backoff in milliseconds (default 100).
	pub bind_backoff_initial_ms: u32,
	/// `VANE_BIND_BACKOFF_MAX_MS` — retry backoff cap in milliseconds (default 5000).
	pub bind_backoff_max_ms: u32,
	/// `VANE_FORCE_CANCEL_GRACE_SECS` — secondary grace window after `force_cancel` fires,
	/// seconds (default 5). Applies to both SIGTERM drain and removed-listener reconcile.
	pub force_cancel_grace_secs: u32,
	/// `VANE_DRAIN_TIMEOUT_SECS` — in-flight connection drain budget for reload and SIGTERM,
	/// seconds (default 30).
	pub drain_timeout_secs: u32,
	/// `VANE_BOOT_HEALTH_TIMEOUT_SECS` — budget for all listeners to flip `bind_ready`,
	/// seconds (default 60). Partial bind (some bound, some failed) stays a warn.
	pub boot_health_timeout_secs: u32,
	/// `VANE_MGMT_UNIX` — management Unix socket path. Defaults to
	/// `$XDG_RUNTIME_DIR/vaned.sock` when that env var is set, then to
	/// `/run/vaned.sock`. `/tmp/...` is intentionally not the default:
	/// it's world-writable and survives reboots, both of which make it
	/// the wrong place for a privileged control socket.
	pub mgmt_unix: PathBuf,
	/// `VANE_MGMT_HTTP_PORT` — TCP port for the HTTP management transport.
	/// `Some(3333)` by default; an explicit empty string disables the
	/// transport (`None`). Matches `spec/crates/core.md`
	/// § _Config layers_.
	pub mgmt_http_port: Option<u16>,
	/// `VANE_MGMT_HTTP_PUBLIC` — when truthy, bind the HTTP management
	/// port on the wildcard address (`0.0.0.0` / `[::]`). When falsy
	/// (default), bind on loopback. Mandatory pairing with
	/// `mgmt_http_token` is enforced at daemon boot, not here.
	pub mgmt_http_public: bool,
	/// `VANE_MGMT_HTTP_TOKEN` — bearer token for the HTTP management
	/// transport (`None` when unset or empty string).
	pub mgmt_http_token: Option<String>,
	/// `VANE_NATIVE_ROOTS_REFRESH_INTERVAL_SECS` — cadence at which
	/// the daemon re-reads the OS native trust store, in seconds
	/// (default 21 600 = 6h). The refresh is non-blocking; failures
	/// preserve the previous snapshot and emit a warn. Operators who
	/// want a one-shot refresh use the `reload_native_roots` mgmt
	/// verb instead.
	pub native_roots_refresh_interval_secs: u32,
}

impl Env {
	/// Read from the actual process environment.
	///
	/// `config_dir` is the daemon's resolved `--config` path; it is
	/// the basis for `wasm_dir`'s default when `VANE_WASM_DIR` is unset.
	///
	/// # Errors
	/// Returns [`Error::compile`] when any `VANE_*` value fails its
	/// type-specific parse (bool, u32, port).
	pub fn from_process_env(config_dir: &Path) -> Result<Self, Error> {
		Self::from_reader(&ProcessEnv, config_dir)
	}

	/// Read from any [`EnvReader`]. Primary entry point for unit tests.
	///
	/// # Errors
	/// As [`Self::from_process_env`].
	pub fn from_reader<R: EnvReader>(r: &R, config_dir: &Path) -> Result<Self, Error> {
		let wasm_dir = r.get("VANE_WASM_DIR").map_or_else(|| config_dir.join("wasm"), PathBuf::from);
		Ok(Self {
			wasm_dir,
			log_level: r
				.get("VANE_LOG_LEVEL")
				.filter(|s| !s.is_empty())
				.unwrap_or_else(|| "info".to_string()),
			bind_ipv4: parse_bool_default_true(r, "VANE_BIND_IPV4")?,
			bind_ipv6: parse_bool_default_true(r, "VANE_BIND_IPV6")?,
			sec_max_header_bytes: parse_u32_default(r, "VANE_SEC_MAX_HEADER_BYTES", 65_536)?,
			sec_max_headers_count: parse_u32_default(r, "VANE_SEC_MAX_HEADERS_COUNT", 100)?,
			sec_header_timeout_secs: parse_u32_default(r, "VANE_SEC_HEADER_TIMEOUT", 30)?,
			sec_max_conn_per_ip: parse_u32_default(r, "VANE_SEC_MAX_CONN_PER_IP", 100)?,
			sec_max_total_conns: parse_u32_default(r, "VANE_SEC_MAX_TOTAL_CONNS", 65_536)?,
			bind_max_attempts: parse_u32_default(r, "VANE_BIND_MAX_ATTEMPTS", 10)?,
			bind_backoff_initial_ms: parse_u32_default(r, "VANE_BIND_BACKOFF_INITIAL_MS", 100)?,
			bind_backoff_max_ms: parse_u32_default(r, "VANE_BIND_BACKOFF_MAX_MS", 5_000)?,
			force_cancel_grace_secs: parse_u32_default(r, "VANE_FORCE_CANCEL_GRACE_SECS", 5)?,
			drain_timeout_secs: parse_u32_default(r, "VANE_DRAIN_TIMEOUT_SECS", 30)?,
			boot_health_timeout_secs: parse_u32_default(r, "VANE_BOOT_HEALTH_TIMEOUT_SECS", 60)?,
			mgmt_unix: r
				.get("VANE_MGMT_UNIX")
				.filter(|s| !s.is_empty())
				.map_or_else(|| default_mgmt_unix(r), PathBuf::from),
			mgmt_http_port: parse_http_port(r)?,
			mgmt_http_public: parse_truthy(r, "VANE_MGMT_HTTP_PUBLIC"),
			mgmt_http_token: r.get("VANE_MGMT_HTTP_TOKEN").filter(|s| !s.is_empty()),
			native_roots_refresh_interval_secs: parse_u32_default(
				r,
				"VANE_NATIVE_ROOTS_REFRESH_INTERVAL_SECS",
				21_600,
			)?,
		})
	}
}

/// Resolve the default management socket path when no `VANE_MGMT_UNIX`
/// is set. Preference order:
///
/// 1. `$XDG_RUNTIME_DIR/vaned.sock` — per-user transient directory
///    that systemd manages with 0700 perms; right place for an
///    unprivileged daemon's control socket.
/// 2. `/run/vaned.sock` — system-wide transient directory; right
///    place for a privileged daemon running under PID 1.
///
/// `/tmp` is never the default: it's world-writable, world-readable
/// in many distros, and survives reboots — any one of which makes
/// it the wrong host for a control socket.
fn default_mgmt_unix<R: EnvReader>(r: &R) -> PathBuf {
	if let Some(dir) = r.get("XDG_RUNTIME_DIR").filter(|s| !s.is_empty()) {
		return PathBuf::from(dir).join("vaned.sock");
	}
	PathBuf::from("/run/vaned.sock")
}

fn parse_bool_default_true<R: EnvReader>(r: &R, key: &str) -> Result<bool, Error> {
	match r.get(key).as_deref() {
		None | Some("" | "1") => Ok(true),
		Some("0") => Ok(false),
		Some(other) => Err(Error::compile(format!("{key} must be \"0\" or \"1\", got {other:?}"))),
	}
}

fn parse_u32_default<R: EnvReader>(r: &R, key: &str, default: u32) -> Result<u32, Error> {
	match r.get(key).filter(|s| !s.is_empty()) {
		None => Ok(default),
		Some(s) => s.parse::<u32>().map_err(|e| Error::compile(format!("{key}: {e} ({s:?})"))),
	}
}

/// Parse `VANE_MGMT_HTTP_PORT`. Unset → default `Some(3333)`; explicit
/// empty string → `None` (transport disabled). Anything else parses as
/// a `u16`.
fn parse_http_port<R: EnvReader>(r: &R) -> Result<Option<u16>, Error> {
	match r.get("VANE_MGMT_HTTP_PORT").as_deref() {
		None => Ok(Some(3333)),
		Some("") => Ok(None),
		Some(s) => s
			.parse::<u16>()
			.map(Some)
			.map_err(|e| Error::compile(format!("VANE_MGMT_HTTP_PORT: {e} ({s:?})"))),
	}
}

/// Boolean env-var parse used for `VANE_MGMT_HTTP_PUBLIC`. Truthy =
/// `1` / `true` / `yes` / `on` (case-insensitive). Anything else,
/// including unset / empty / `0` / `false` / `no` / `off`, is falsy.
fn parse_truthy<R: EnvReader>(r: &R, key: &str) -> bool {
	matches!(r.get(key).map(|s| s.to_ascii_lowercase()).as_deref(), Some("1" | "true" | "yes" | "on"),)
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;

	use super::*;

	struct FakeEnv(HashMap<&'static str, &'static str>);

	impl FakeEnv {
		fn empty() -> Self {
			Self(HashMap::new())
		}

		fn with(pairs: &[(&'static str, &'static str)]) -> Self {
			Self(pairs.iter().copied().collect())
		}
	}

	impl EnvReader for FakeEnv {
		fn get(&self, key: &str) -> Option<String> {
			self.0.get(key).map(|s| (*s).to_string())
		}
	}

	fn cfg() -> PathBuf {
		PathBuf::from("/etc/vaned")
	}

	#[test]
	fn env_defaults_when_all_unset() {
		let env = Env::from_reader(&FakeEnv::empty(), &cfg()).expect("defaults");
		assert_eq!(env.log_level, "info");
		assert!(env.bind_ipv4);
		assert!(env.bind_ipv6);
		assert_eq!(env.sec_max_header_bytes, 65_536);
		assert_eq!(env.sec_max_headers_count, 100);
		assert_eq!(env.sec_header_timeout_secs, 30);
		assert_eq!(env.sec_max_conn_per_ip, 100);
		assert_eq!(env.sec_max_total_conns, 65_536);
		// `/run/vaned.sock` is the no-XDG_RUNTIME_DIR fallback. `/tmp`
		// is never the default — see `default_mgmt_unix` for rationale.
		assert_eq!(env.mgmt_unix, PathBuf::from("/run/vaned.sock"));
		assert_eq!(env.mgmt_http_port, Some(3333));
		assert!(!env.mgmt_http_public);
		assert!(env.mgmt_http_token.is_none());
	}

	#[test]
	fn env_mgmt_unix_prefers_xdg_runtime_dir_when_set() {
		let env = Env::from_reader(&FakeEnv::with(&[("XDG_RUNTIME_DIR", "/run/user/1000")]), &cfg())
			.expect("ok");
		assert_eq!(env.mgmt_unix, PathBuf::from("/run/user/1000/vaned.sock"));
	}

	#[test]
	fn env_bind_ipv4_zero_yields_false() {
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_BIND_IPV4", "0")]), &cfg()).expect("ok");
		assert!(!env.bind_ipv4);
	}

	#[test]
	fn env_bind_ipv4_one_yields_true() {
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_BIND_IPV4", "1")]), &cfg()).expect("ok");
		assert!(env.bind_ipv4);
	}

	#[test]
	fn env_bind_ipv4_empty_string_falls_back_to_default() {
		// dotenvy may write `KEY=` with no value — that should not be a
		// hard error; treat as unset.
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_BIND_IPV4", "")]), &cfg()).expect("ok");
		assert!(env.bind_ipv4, "empty string falls back to default true");
	}

	#[test]
	fn env_bind_ipv4_invalid_returns_compile_error_naming_var() {
		let err =
			Env::from_reader(&FakeEnv::with(&[("VANE_BIND_IPV4", "yes")]), &cfg()).expect_err("invalid");
		let msg = err.to_string();
		assert!(msg.contains("VANE_BIND_IPV4"), "error names the var: {msg}");
		assert!(msg.contains("\"yes\""), "error quotes the offending value: {msg}");
	}

	#[test]
	fn env_sec_integers_parse() {
		let env = Env::from_reader(
			&FakeEnv::with(&[
				("VANE_SEC_MAX_HEADER_BYTES", "32768"),
				("VANE_SEC_MAX_HEADERS_COUNT", "64"),
				("VANE_SEC_HEADER_TIMEOUT", "10"),
				("VANE_SEC_MAX_CONN_PER_IP", "500"),
			]),
			&cfg(),
		)
		.expect("ok");
		assert_eq!(env.sec_max_header_bytes, 32_768);
		assert_eq!(env.sec_max_headers_count, 64);
		assert_eq!(env.sec_header_timeout_secs, 10);
		assert_eq!(env.sec_max_conn_per_ip, 500);
	}

	#[test]
	fn env_sec_invalid_integer_errors() {
		let err = Env::from_reader(&FakeEnv::with(&[("VANE_SEC_MAX_HEADER_BYTES", "huge")]), &cfg())
			.expect_err("non-int rejected");
		let msg = err.to_string();
		assert!(msg.contains("VANE_SEC_MAX_HEADER_BYTES"), "{msg}");
	}

	#[test]
	fn env_sec_negative_integer_errors() {
		// u32 cannot hold negative; ensure the error path fires cleanly.
		let err = Env::from_reader(&FakeEnv::with(&[("VANE_SEC_MAX_CONN_PER_IP", "-1")]), &cfg())
			.expect_err("negative rejected");
		assert!(err.to_string().contains("VANE_SEC_MAX_CONN_PER_IP"));
	}

	#[test]
	fn env_mgmt_http_port_default_is_3333() {
		let env = Env::from_reader(&FakeEnv::empty(), &cfg()).expect("defaults");
		assert_eq!(env.mgmt_http_port, Some(3333));
	}

	#[test]
	fn env_mgmt_http_port_empty_string_disables_transport() {
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_PORT", "")]), &cfg()).expect("ok");
		assert_eq!(env.mgmt_http_port, None);
	}

	#[test]
	fn env_mgmt_http_port_explicit_value_parses() {
		let env =
			Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_PORT", "9000")]), &cfg()).expect("ok");
		assert_eq!(env.mgmt_http_port, Some(9000));
	}

	#[test]
	fn env_mgmt_http_port_invalid_errors() {
		let err = Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_PORT", "nope")]), &cfg())
			.expect_err("bad port");
		let msg = err.to_string();
		assert!(msg.contains("VANE_MGMT_HTTP_PORT"), "{msg}");
		assert!(msg.contains("\"nope\""), "{msg}");
	}

	#[test]
	fn env_mgmt_http_public_truthy_values() {
		for v in ["1", "true", "TRUE", "Yes", "on"] {
			let env =
				Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_PUBLIC", v)]), &cfg()).expect("ok");
			assert!(env.mgmt_http_public, "{v} should be truthy");
		}
	}

	#[test]
	fn env_mgmt_http_public_falsy_values() {
		for v in ["", "0", "false", "no", "off"] {
			let env =
				Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_PUBLIC", v)]), &cfg()).expect("ok");
			assert!(!env.mgmt_http_public, "{v} should be falsy");
		}
	}

	#[test]
	fn env_mgmt_http_token_empty_string_yields_none() {
		let env =
			Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_TOKEN", "")]), &cfg()).expect("ok");
		assert!(env.mgmt_http_token.is_none());
	}

	#[test]
	fn env_mgmt_http_token_value_passes_through() {
		let env =
			Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_TOKEN", "hunter2")]), &cfg()).expect("ok");
		assert_eq!(env.mgmt_http_token.as_deref(), Some("hunter2"));
	}

	#[test]
	fn env_mgmt_unix_default_path() {
		let env = Env::from_reader(&FakeEnv::empty(), &cfg()).expect("defaults");
		assert_eq!(env.mgmt_unix, PathBuf::from("/run/vaned.sock"));
	}

	#[test]
	fn env_mgmt_unix_override() {
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_UNIX", "/run/vane.sock")]), &cfg())
			.expect("ok");
		assert_eq!(env.mgmt_unix, PathBuf::from("/run/vane.sock"));
	}

	#[test]
	fn env_log_level_passes_through_verbatim() {
		for level in ["debug", "warn", "trace", "vane=info,hyper=warn"] {
			let env = Env::from_reader(&FakeEnv::with(&[("VANE_LOG_LEVEL", level)]), &cfg()).expect("ok");
			assert_eq!(env.log_level, level);
		}
	}

	#[test]
	fn env_wasm_dir_defaults_to_clap_config_dir_subdir() {
		let env = Env::from_reader(&FakeEnv::empty(), &cfg()).expect("defaults");
		assert_eq!(env.wasm_dir, PathBuf::from("/etc/vaned/wasm"));

		let env = Env::from_reader(&FakeEnv::empty(), &PathBuf::from("/srv/vane/etc"))
			.expect("custom config_dir");
		assert_eq!(
			env.wasm_dir,
			PathBuf::from("/srv/vane/etc/wasm"),
			"default tracks the supplied config_dir",
		);
	}

	#[test]
	fn env_wasm_dir_explicit_override_wins() {
		let env =
			Env::from_reader(&FakeEnv::with(&[("VANE_WASM_DIR", "/var/lib/vane/plugins")]), &cfg())
				.expect("override");
		assert_eq!(env.wasm_dir, PathBuf::from("/var/lib/vane/plugins"));
	}
}
