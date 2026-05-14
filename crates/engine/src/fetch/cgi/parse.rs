//! Args parsing, validation, and the public `factory` entry point.
//! Runs at link time per `FetchInst` construction.

use std::path::PathBuf;
use std::sync::Arc;

use cgi_request::is_reserved_env_key;
use serde_json::Value;

use super::{
	CgiArgs, CgiSecurity, CgiTimeouts, DEFAULT_CONNECT_TIMEOUT, DEFAULT_TOTAL_TIMEOUT, ResourceLimits,
};
use crate::factories::FactoryError;
use crate::flow_graph::FetchInst;

/// Build a `CgiFetch` from the resolved rule args.
///
/// # Errors
/// Returns [`FactoryError`] when any required field is missing or
/// malformed, when `security.chroot` is set (reserved but not yet
/// implemented), when the `binary` path does not exist or is not a
/// regular file, when the binary is not executable by the configured
/// `uid`, or when `args.env` contains a reserved key.
#[cfg(unix)]
pub fn factory(args: &Value) -> Result<FetchInst, FactoryError> {
	let parsed = parse_args(args).map_err(FactoryError::Invalid)?;
	validate_binary(&parsed.binary, &parsed.security).map_err(FactoryError::Invalid)?;
	if parsed.security.uid == 0 {
		tracing::warn!(
			binary = %parsed.binary.display(),
			"cgi rule configured to run as root; verify this is intended",
		);
	}
	Ok(FetchInst::L7(Arc::new(super::runtime::CgiFetch { args: parsed })))
}

/// Non-unix stub. The CGI driver is unix-only — `pre_exec` is the
/// load-bearing primitive and has no Windows / WASI analogue.
#[cfg(not(unix))]
pub fn factory(_args: &Value) -> Result<FetchInst, FactoryError> {
	Err(FactoryError::Invalid("CGI fetch driver is unix-only".to_string()))
}

fn parse_args(args: &Value) -> Result<CgiArgs, String> {
	let obj = args.as_object().ok_or_else(|| "args must be a JSON object".to_string())?;

	let binary = require_path(obj, "binary")?;
	let script_name = require_string(obj, "script_name")?;
	let working_dir = require_path(obj, "working_dir")?;
	let env = parse_env(obj)?;
	let block_headers = parse_block_headers(obj)?;
	let security = parse_security(obj)?;
	let timeouts = parse_timeouts(obj)?;

	Ok(CgiArgs { binary, script_name, working_dir, env, block_headers, security, timeouts })
}

fn require_string(obj: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
	obj
		.get(key)
		.ok_or_else(|| format!("missing args.{key}"))?
		.as_str()
		.map(str::to_owned)
		.ok_or_else(|| format!("args.{key} must be a string"))
}

fn require_path(obj: &serde_json::Map<String, Value>, key: &str) -> Result<PathBuf, String> {
	require_string(obj, key).map(PathBuf::from)
}

fn parse_env(obj: &serde_json::Map<String, Value>) -> Result<Vec<(String, String)>, String> {
	let raw = obj.get("env").ok_or_else(|| "missing args.env (object, may be empty)".to_string())?;
	let map =
		raw.as_object().ok_or_else(|| "args.env must be a JSON object (key→value)".to_string())?;
	let mut out = Vec::with_capacity(map.len());
	for (k, v) in map {
		if is_reserved_env_key(k) {
			return Err(format!(
				"args.env key {k:?} is reserved (RFC 3875 / common extension / HTTP_*); operators \
				 cannot override values vane computes per request — see `spec/crates/engine.md` § _CGI_"
			));
		}
		let val = v.as_str().ok_or_else(|| format!("args.env[{k:?}] must be a string, got {v:?}"))?;
		out.push((k.clone(), val.to_owned()));
	}
	Ok(out)
}

fn parse_block_headers(obj: &serde_json::Map<String, Value>) -> Result<Vec<String>, String> {
	let raw = obj
		.get("block_headers")
		.ok_or_else(|| "missing args.block_headers (list, may be empty)".to_string())?;
	let arr = raw
		.as_array()
		.ok_or_else(|| "args.block_headers must be a JSON array of strings".to_string())?;
	let mut out = Vec::with_capacity(arr.len());
	for entry in arr {
		let s = entry
			.as_str()
			.ok_or_else(|| format!("args.block_headers entries must be strings, got {entry:?}"))?;
		out.push(s.to_owned());
	}
	Ok(out)
}

fn parse_security(obj: &serde_json::Map<String, Value>) -> Result<CgiSecurity, String> {
	let raw = obj
		.get("security")
		.ok_or_else(|| "missing args.security (object)".to_string())?
		.as_object()
		.ok_or_else(|| "args.security must be a JSON object".to_string())?;
	let uid = require_u32(raw, "security.uid")?;
	let gid = require_u32(raw, "security.gid")?;

	// `chroot` is reserved at the schema level so the JSON shape stays
	// stable for a future post-MVP implementation pass. `spec/crates/engine.md` § _Security_:
	// "a CGI rule with chroot: Some(...) fails compile with 'chroot is
	// reserved but not yet implemented'."
	let chroot = raw
		.get("chroot")
		.ok_or_else(|| "missing args.security.chroot (use null to skip)".to_string())?;
	if !chroot.is_null() {
		return Err("chroot is reserved but not yet implemented".to_string());
	}

	let limits_raw = raw
		.get("limits")
		.ok_or_else(|| "missing args.security.limits (object)".to_string())?
		.as_object()
		.ok_or_else(|| "args.security.limits must be a JSON object".to_string())?;
	let limits = ResourceLimits {
		memory_mb: require_optional_u64(limits_raw, "security.limits.memory_mb")?,
		cpu_seconds: require_optional_u64(limits_raw, "security.limits.cpu_seconds")?,
		max_processes: require_optional_u64(limits_raw, "security.limits.max_processes")?,
	};
	Ok(CgiSecurity { uid, gid, limits })
}

fn require_u32(obj: &serde_json::Map<String, Value>, key: &str) -> Result<u32, String> {
	let v = obj
		.get(key.rsplit_once('.').map_or(key, |(_, t)| t))
		.ok_or_else(|| format!("missing args.{key}"))?;
	let n = v.as_u64().ok_or_else(|| format!("args.{key} must be an unsigned integer"))?;
	u32::try_from(n).map_err(|_| format!("args.{key} must fit in u32"))
}

/// Parse a "must be present, may be `null`" numeric field. Spec rule:
/// each `security.limits.*` field must appear in the JSON, and `null`
/// is the explicit "no limit" choice (operators must opt out
/// consciously rather than by omission).
fn require_optional_u64(
	obj: &serde_json::Map<String, Value>,
	key: &str,
) -> Result<Option<u64>, String> {
	let leaf = key.rsplit_once('.').map_or(key, |(_, t)| t);
	let v = obj.get(leaf).ok_or_else(|| format!("missing args.{key} (use null for no limit)"))?;
	if v.is_null() {
		return Ok(None);
	}
	let n = v.as_u64().ok_or_else(|| format!("args.{key} must be a non-negative integer or null"))?;
	Ok(Some(n))
}

fn parse_timeouts(obj: &serde_json::Map<String, Value>) -> Result<CgiTimeouts, String> {
	let raw = obj.get("timeouts");
	let (connect, total) = match raw {
		None => (DEFAULT_CONNECT_TIMEOUT, DEFAULT_TOTAL_TIMEOUT),
		Some(v) => {
			let m = v.as_object().ok_or_else(|| "args.timeouts must be a JSON object".to_string())?;
			let connect = match m.get("connect") {
				Some(s) => crate::fetch::retry::parse_duration(
					s.as_str().ok_or_else(|| "args.timeouts.connect must be a string".to_string())?,
				)
				.map_err(|e| format!("args.timeouts.connect: {e}"))?,
				None => DEFAULT_CONNECT_TIMEOUT,
			};
			let total = match m.get("total") {
				Some(s) => crate::fetch::retry::parse_duration(
					s.as_str().ok_or_else(|| "args.timeouts.total must be a string".to_string())?,
				)
				.map_err(|e| format!("args.timeouts.total: {e}"))?,
				None => DEFAULT_TOTAL_TIMEOUT,
			};
			(connect, total)
		}
	};
	Ok(CgiTimeouts { connect, total })
}

/// Bootstrap validation per `spec/crates/engine.md` § _Bootstrap validation_: rule-level
/// compile error (not daemon-wide) when the binary is missing /
/// non-file / non-executable for the configured uid.
//
// `similar_names` flags `file_uid` / `file_gid` against each other —
// they're a pair of related fields and the close naming is the clearer
// expression here.
#[cfg(unix)]
fn validate_binary(binary: &std::path::Path, security: &CgiSecurity) -> Result<(), String> {
	use std::os::unix::fs::MetadataExt as _;
	let path_display = binary.display();
	let meta =
		std::fs::metadata(binary).map_err(|e| format!("binary {path_display} not accessible: {e}"))?;
	if !meta.is_file() {
		return Err(format!("binary {path_display} is not a regular file"));
	}
	// Spec wants `access(2) X_OK` evaluated against the target uid;
	// the prompt explicitly permits the simpler "stat + check uid /
	// gid / other X bits" approach. Accuracy-wise this matches the
	// kernel's check for the case the prompt cares about (rule-level
	// catch of obvious typos / missing binaries) — it does not handle
	// ACLs (`getfacl`) or capability-based execution (which are out of
	// scope for the rule-level check; the kernel will surface those at
	// `execve` time as an `EACCES` from `spawn()`).
	let mode = meta.mode();
	let file_uid = meta.uid();
	let file_gid = meta.gid();
	let executable = if file_uid == security.uid {
		(mode & 0o100) != 0
	} else if file_gid == security.gid {
		(mode & 0o010) != 0
	} else {
		(mode & 0o001) != 0
	};
	if !executable {
		return Err(format!(
			"binary {path_display} (mode {mode:o}, owner {file_uid}:{file_gid}) is not executable by \
			 uid {} / gid {} configured for this rule",
			security.uid, security.gid,
		));
	}
	Ok(())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
	use std::io::Write as _;
	use std::os::unix::fs::PermissionsExt as _;

	use serde_json::json;
	use tempfile::NamedTempFile;

	use super::*;

	/// Minimal-valid args: `binary` is a real chmod 0o755 file owned
	/// by the test process (so the validator's "executable by uid"
	/// check passes against the current uid).
	fn fixture_binary() -> NamedTempFile {
		let mut f = NamedTempFile::new().expect("tmp");
		f.write_all(b"#!/bin/sh\necho ok\n").expect("write");
		let p = f.path();
		std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).expect("chmod 755");
		f
	}

	/// Read the current process's effective uid via `stat()` on a
	/// freshly created temp file (whose owner is the calling uid).
	/// Avoids reaching for `libc::getuid` here because the workspace
	/// `unsafe_code = deny` lint forbids unsafe blocks until the
	/// runtime commit's audited `pre_exec` closure lands.
	fn current_uid() -> u32 {
		use std::os::unix::fs::MetadataExt as _;
		let f = NamedTempFile::new().expect("probe tmp");
		std::fs::metadata(f.path()).expect("probe stat").uid()
	}

	fn current_gid() -> u32 {
		use std::os::unix::fs::MetadataExt as _;
		let f = NamedTempFile::new().expect("probe tmp");
		std::fs::metadata(f.path()).expect("probe stat").gid()
	}

	fn expect_factory_err(args: &Value) -> FactoryError {
		// `FetchInst` deliberately doesn't impl `Debug` (it carries
		// trait objects), so we can't use `.expect_err` directly.
		match factory(args) {
			Ok(_) => panic!("expected FactoryError, got Ok"),
			Err(e) => e,
		}
	}

	fn minimal_valid_args(bin: &std::path::Path) -> Value {
		json!({
			"upstream_kind": "cgi",
			"binary": bin.to_str().unwrap(),
			"script_name": "/cgi-bin/app.cgi",
			"working_dir": bin.parent().unwrap().to_str().unwrap(),
			"env": {},
			"block_headers": ["Authorization", "Cookie", "Proxy-Authorization"],
			"security": {
				"uid": current_uid(),
				"gid": current_gid(),
				"limits": { "memory_mb": null, "cpu_seconds": null, "max_processes": null },
				"chroot": null,
			},
		})
	}

	#[test]
	fn factory_accepts_minimal_valid_args() {
		let bin = fixture_binary();
		let args = minimal_valid_args(bin.path());
		let inst = factory(&args).expect("minimal valid args must parse");
		assert!(matches!(inst, FetchInst::L7(_)));
	}

	#[test]
	fn factory_rejects_missing_binary_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("binary");
		let err = expect_factory_err(&args);
		assert!(err.message().contains("binary"), "error must name the missing field: {err:?}");
	}

	#[test]
	fn factory_rejects_missing_script_name_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("script_name");
		let err = expect_factory_err(&args);
		assert!(err.message().contains("script_name"), "must name field: {err:?}");
	}

	#[test]
	fn factory_rejects_missing_working_dir_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("working_dir");
		let err = expect_factory_err(&args);
		assert!(err.message().contains("working_dir"), "must name field: {err:?}");
	}

	#[test]
	fn factory_rejects_missing_env_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("env");
		let err = expect_factory_err(&args);
		assert!(err.message().contains("env"), "must name field: {err:?}");
	}

	#[test]
	fn factory_rejects_missing_block_headers_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("block_headers");
		let err = expect_factory_err(&args);
		assert!(err.message().contains("block_headers"), "must name field: {err:?}");
	}

	#[test]
	fn factory_rejects_chroot_some_with_spec_text() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["security"]["chroot"] = json!("/var/empty");
		let err = expect_factory_err(&args);
		assert!(
			err.message().contains("chroot is reserved but not yet implemented"),
			"must use spec wording verbatim: {err:?}",
		);
	}

	#[test]
	fn factory_rejects_security_limits_missing_field_not_null() {
		// Field absence is an error; null (= "no limit") is allowed.
		// This locks the spec rule that operators must consciously opt
		// out of each limit rather than getting it for free.
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["security"]["limits"].as_object_mut().unwrap().remove("memory_mb");
		let err = expect_factory_err(&args);
		assert!(err.message().contains("memory_mb"), "must name field: {err:?}");
	}

	#[test]
	fn factory_accepts_security_limits_with_null_for_no_limit() {
		// Null is the "no limit" sentinel — explicitly accepted.
		let bin = fixture_binary();
		let args = minimal_valid_args(bin.path());
		factory(&args).expect("null limit must parse");
	}

	#[test]
	fn factory_accepts_security_limits_with_integer_value() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["security"]["limits"]["memory_mb"] = json!(256);
		args["security"]["limits"]["cpu_seconds"] = json!(30);
		factory(&args).expect("integer limits must parse");
	}

	#[test]
	fn factory_rejects_env_with_reserved_request_method_key() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["env"] = json!({ "REQUEST_METHOD": "FAKE" });
		let err = expect_factory_err(&args);
		assert!(
			err.message().contains("REQUEST_METHOD"),
			"must name the offending key: {}",
			err.message()
		);
		assert!(err.message().contains("reserved"), "must explain why: {err:?}");
	}

	#[test]
	fn factory_rejects_env_with_reserved_http_prefixed_key() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["env"] = json!({ "HTTP_USER_AGENT": "x" });
		let err = expect_factory_err(&args);
		assert!(
			err.message().contains("HTTP_USER_AGENT"),
			"must name the offending key: {}",
			err.message()
		);
	}

	#[test]
	fn factory_rejects_env_with_reserved_common_extension_key() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["env"] = json!({ "HTTPS": "on" });
		let err = expect_factory_err(&args);
		assert!(err.message().contains("HTTPS"), "must name the offending key: {}", err.message());
	}

	#[test]
	fn factory_rejects_binary_that_does_not_exist() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["binary"] = json!("/no/such/path/here-cgi-fixture");
		let err = expect_factory_err(&args);
		assert!(err.message().contains("not accessible"), "must explain: {err:?}");
	}

	#[test]
	fn factory_rejects_binary_that_is_a_directory() {
		let bin = fixture_binary();
		let dir = bin.path().parent().unwrap();
		let mut args = minimal_valid_args(bin.path());
		args["binary"] = json!(dir.to_str().unwrap());
		let err = expect_factory_err(&args);
		assert!(err.message().contains("not a regular file"), "must explain: {err:?}");
	}

	#[test]
	fn factory_rejects_binary_without_executable_bit() {
		let mut f = NamedTempFile::new().expect("tmp");
		f.write_all(b"plain").expect("write");
		std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o644)).expect("chmod 644");
		let mut args = minimal_valid_args(f.path());
		// Force the validator down the "owner" branch by claiming the
		// current uid (the file's owner). Owner bits are 644 — no x.
		args["security"]["uid"] = json!(current_uid());
		let err = expect_factory_err(&args);
		assert!(err.message().contains("not executable"), "must explain: {err:?}");
	}

	#[test]
	fn is_reserved_env_key_recognises_each_set() {
		assert!(is_reserved_env_key("REQUEST_METHOD"));
		assert!(is_reserved_env_key("HTTPS"));
		assert!(is_reserved_env_key("HTTP_USER_AGENT"));
		assert!(!is_reserved_env_key("DATABASE_URL"));
		assert!(!is_reserved_env_key("APP_MODE"));
	}
}
