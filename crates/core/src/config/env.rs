//! Typed accessors for `VANE_*` deployment-constant env vars
//! (`spec/architecture/09-config.md` § _Three-layer configuration_).
//!
//! The [`EnvReader`] trait abstracts the source so unit tests pass a
//! `HashMap`-backed fake instead of mutating process-global state — Rust
//! 1.95 marks `std::env::set_var` `unsafe` due to multi-thread races.

use std::net::SocketAddr;
use std::path::PathBuf;

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
/// reads at startup. Defaults match `spec/architecture/09-config.md`
/// § _Three-layer configuration_.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
	/// `VANE_DATA_DIR` — daemon working data root (default `/var/lib/vaned`).
	pub data_dir: PathBuf,
	/// `VANE_CONFIG_DIR` — config-tree root (default `/etc/vaned`).
	pub config_dir: PathBuf,
	/// `VANE_LOG_LEVEL` — passed through to the tracing subscriber
	/// verbatim (default `"info"`).
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
	/// `VANE_MGMT_UNIX` — management Unix socket path (default `/var/run/vaned.sock`).
	pub mgmt_unix: PathBuf,
	/// `VANE_MGMT_HTTP_BIND` — optional HTTP management endpoint (`None`
	/// when unset or empty string; otherwise must parse as `SocketAddr`).
	pub mgmt_http_bind: Option<SocketAddr>,
	/// `VANE_MGMT_HTTP_TOKEN` — bearer token for `mgmt_http_bind` (`None`
	/// when unset or empty string).
	pub mgmt_http_token: Option<String>,
}

impl Env {
	/// Read from the actual process environment.
	///
	/// # Errors
	/// Returns [`Error::compile`] when any `VANE_*` value fails its
	/// type-specific parse (bool, u32, `SocketAddr`).
	pub fn from_process_env() -> Result<Self, Error> {
		Self::from_reader(&ProcessEnv)
	}

	/// Read from any [`EnvReader`]. Primary entry point for unit tests.
	///
	/// # Errors
	/// As [`Self::from_process_env`].
	pub fn from_reader<R: EnvReader>(r: &R) -> Result<Self, Error> {
		Ok(Self {
			data_dir: r
				.get("VANE_DATA_DIR")
				.map_or_else(|| PathBuf::from("/var/lib/vaned"), PathBuf::from),
			config_dir: r
				.get("VANE_CONFIG_DIR")
				.map_or_else(|| PathBuf::from("/etc/vaned"), PathBuf::from),
			log_level: r.get("VANE_LOG_LEVEL").unwrap_or_else(|| "info".to_string()),
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
				.map_or_else(|| PathBuf::from("/var/run/vaned.sock"), PathBuf::from),
			mgmt_http_bind: parse_socket_addr_optional(r, "VANE_MGMT_HTTP_BIND")?,
			mgmt_http_token: r.get("VANE_MGMT_HTTP_TOKEN").filter(|s| !s.is_empty()),
		})
	}
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

fn parse_socket_addr_optional<R: EnvReader>(r: &R, key: &str) -> Result<Option<SocketAddr>, Error> {
	match r.get(key).filter(|s| !s.is_empty()) {
		None => Ok(None),
		Some(s) => s
			.parse::<SocketAddr>()
			.map(Some)
			.map_err(|e| Error::compile(format!("{key}: invalid SocketAddr {s:?}: {e}"))),
	}
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

	#[test]
	fn env_defaults_when_all_unset() {
		let env = Env::from_reader(&FakeEnv::empty()).expect("defaults");
		assert_eq!(env.data_dir, PathBuf::from("/var/lib/vaned"));
		assert_eq!(env.config_dir, PathBuf::from("/etc/vaned"));
		assert_eq!(env.log_level, "info");
		assert!(env.bind_ipv4);
		assert!(env.bind_ipv6);
		assert_eq!(env.sec_max_header_bytes, 65_536);
		assert_eq!(env.sec_max_headers_count, 100);
		assert_eq!(env.sec_header_timeout_secs, 30);
		assert_eq!(env.sec_max_conn_per_ip, 100);
		assert_eq!(env.sec_max_total_conns, 65_536);
		assert_eq!(env.mgmt_unix, PathBuf::from("/var/run/vaned.sock"));
		assert!(env.mgmt_http_bind.is_none());
		assert!(env.mgmt_http_token.is_none());
	}

	#[test]
	fn env_bind_ipv4_zero_yields_false() {
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_BIND_IPV4", "0")])).expect("ok");
		assert!(!env.bind_ipv4);
	}

	#[test]
	fn env_bind_ipv4_one_yields_true() {
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_BIND_IPV4", "1")])).expect("ok");
		assert!(env.bind_ipv4);
	}

	#[test]
	fn env_bind_ipv4_empty_string_falls_back_to_default() {
		// dotenvy may write `KEY=` with no value — that should not be a
		// hard error; treat as unset.
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_BIND_IPV4", "")])).expect("ok");
		assert!(env.bind_ipv4, "empty string falls back to default true");
	}

	#[test]
	fn env_bind_ipv4_invalid_returns_compile_error_naming_var() {
		let err = Env::from_reader(&FakeEnv::with(&[("VANE_BIND_IPV4", "yes")])).expect_err("invalid");
		let msg = err.to_string();
		assert!(msg.contains("VANE_BIND_IPV4"), "error names the var: {msg}");
		assert!(msg.contains("\"yes\""), "error quotes the offending value: {msg}");
	}

	#[test]
	fn env_sec_integers_parse() {
		let env = Env::from_reader(&FakeEnv::with(&[
			("VANE_SEC_MAX_HEADER_BYTES", "32768"),
			("VANE_SEC_MAX_HEADERS_COUNT", "64"),
			("VANE_SEC_HEADER_TIMEOUT", "10"),
			("VANE_SEC_MAX_CONN_PER_IP", "500"),
		]))
		.expect("ok");
		assert_eq!(env.sec_max_header_bytes, 32_768);
		assert_eq!(env.sec_max_headers_count, 64);
		assert_eq!(env.sec_header_timeout_secs, 10);
		assert_eq!(env.sec_max_conn_per_ip, 500);
	}

	#[test]
	fn env_sec_invalid_integer_errors() {
		let err = Env::from_reader(&FakeEnv::with(&[("VANE_SEC_MAX_HEADER_BYTES", "huge")]))
			.expect_err("non-int rejected");
		let msg = err.to_string();
		assert!(msg.contains("VANE_SEC_MAX_HEADER_BYTES"), "{msg}");
	}

	#[test]
	fn env_sec_negative_integer_errors() {
		// u32 cannot hold negative; ensure the error path fires cleanly.
		let err = Env::from_reader(&FakeEnv::with(&[("VANE_SEC_MAX_CONN_PER_IP", "-1")]))
			.expect_err("negative rejected");
		assert!(err.to_string().contains("VANE_SEC_MAX_CONN_PER_IP"));
	}

	#[test]
	fn env_mgmt_http_bind_empty_string_yields_none() {
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_BIND", "")])).expect("ok");
		assert!(env.mgmt_http_bind.is_none());
	}

	#[test]
	fn env_mgmt_http_bind_valid_socketaddr_parses() {
		let env =
			Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_BIND", "127.0.0.1:9000")])).expect("ok");
		let addr = env.mgmt_http_bind.expect("set");
		assert_eq!(addr.port(), 9000);
	}

	#[test]
	fn env_mgmt_http_bind_invalid_errors() {
		let err = Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_BIND", "not-an-addr")]))
			.expect_err("bad addr");
		let msg = err.to_string();
		assert!(msg.contains("VANE_MGMT_HTTP_BIND"), "{msg}");
		assert!(msg.contains("\"not-an-addr\""), "error quotes offending value: {msg}");
	}

	#[test]
	fn env_mgmt_http_token_empty_string_yields_none() {
		let env = Env::from_reader(&FakeEnv::with(&[("VANE_MGMT_HTTP_TOKEN", "")])).expect("ok");
		assert!(env.mgmt_http_token.is_none());
	}

	#[test]
	fn env_log_level_passes_through_verbatim() {
		for level in ["debug", "warn", "trace", "vane=info,hyper=warn"] {
			let env = Env::from_reader(&FakeEnv::with(&[("VANE_LOG_LEVEL", level)])).expect("ok");
			assert_eq!(env.log_level, level);
		}
	}

	#[test]
	fn env_data_and_config_dirs_passed_through_as_pathbuf() {
		let env = Env::from_reader(&FakeEnv::with(&[
			("VANE_DATA_DIR", "/srv/vane/data"),
			("VANE_CONFIG_DIR", "/srv/vane/etc"),
		]))
		.expect("ok");
		assert_eq!(env.data_dir, PathBuf::from("/srv/vane/data"));
		assert_eq!(env.config_dir, PathBuf::from("/srv/vane/etc"));
	}
}
