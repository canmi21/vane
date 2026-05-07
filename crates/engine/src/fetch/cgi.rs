//! `CgiFetch` — RFC 3875 CGI driver.
//!
//! Per `spec/crates/engine.md`, every request fork-execs a fresh
//! process, pipes the request body to its stdin, parses the child's
//! stdout as an RFC 3875 response, and emits stderr lines as `tracing`
//! events. The driver lives in its own module rather than the
//! `HttpProxyFetch` `Dispatch` enum because none of the socket-side
//! machinery (connection pool, retry, ALPN, upstream URI rewrite,
//! `connect_timeout` semantics) applies — fork+exec is a different
//! protocol with different invariants.
//!
//! # `unsafe` boundary
//!
//! The `pre_exec` closure passed to
//! `std::os::unix::process::CommandExt::pre_exec` runs in the child
//! process between `fork(2)` and `execve(2)`. POSIX restricts that
//! window to **async-signal-safe** operations only — any allocation,
//! mutex acquisition, or file I/O risks deadlock with whatever the
//! parent was holding when `fork` fired.
//!
//! The closure in this module is the **only** `unsafe` block in the
//! workspace (the workspace-level lint `unsafe_code = "deny"` is
//! lifted here with `#[allow(unsafe_code)]`). The closure is held to
//! the following discipline, audited line by line:
//!
//! * No allocations (no `Vec` / `Box` / `String` construction; no
//!   `format!`).
//! * No mutex locks (no `parking_lot`, no `std::sync::Mutex`).
//! * No file I/O beyond the listed syscalls (`setgid`, `setuid`,
//!   `setrlimit`).
//! * No panics (`?` is fine because the only `Result` it produces is
//!   `io::Error` from a syscall failure).
//! * No `tracing` calls (the macro expansion allocates).
//!
//! Errors from the closure surface to the parent side of `spawn()` as
//! the spawn future's `Err`.
//!
//! Auditor: Canmi

#![allow(unsafe_code)]

use std::future::poll_fn;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderName, HeaderValue, StatusCode};
use http_body::{Body as HttpBody, Frame, SizeHint};
use serde_json::Value;
use tokio::io::{
	AsyncBufReadExt as _, AsyncRead as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader, ReadBuf,
};
use tokio::process::{ChildStdout, Command};
use tokio::sync::Semaphore;
use tokio::time::Instant;
use vane_core::{Body, ConnContext, Error, FlowCtx, L7Fetch, L7FetchOutput, Request, Response};

use crate::factories::FactoryError;
use crate::flow_graph::FetchInst;

/// Resolved per-rule CGI configuration. Built once at link time;
/// `CgiFetch::fetch` reads it on every request.
#[derive(Debug, Clone)]
pub(crate) struct CgiArgs {
	pub binary: PathBuf,
	pub script_name: String,
	pub working_dir: PathBuf,
	pub env: Vec<(String, String)>,
	pub block_headers: Vec<String>,
	pub security: CgiSecurity,
	pub timeouts: CgiTimeouts,
}

#[derive(Debug, Clone)]
pub(crate) struct CgiSecurity {
	pub uid: u32,
	pub gid: u32,
	pub limits: ResourceLimits,
}

#[derive(Debug, Clone)]
pub(crate) struct ResourceLimits {
	pub memory_mb: Option<u64>,
	pub cpu_seconds: Option<u64>,
	pub max_processes: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct CgiTimeouts {
	pub connect: Duration,
	pub total: Duration,
}

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_mins(1);

/// RFC 3875 §4.1 required variables. Operators cannot override these
/// via `args.env` — vane computes them per request from connection /
/// request state. See spec § _User-defined, per rule_.
const RFC_3875_REQUIRED: &[&str] = &[
	"CONTENT_LENGTH",
	"CONTENT_TYPE",
	"GATEWAY_INTERFACE",
	"PATH_INFO",
	"PATH_TRANSLATED",
	"QUERY_STRING",
	"REMOTE_ADDR",
	"REMOTE_HOST",
	"REQUEST_METHOD",
	"SCRIPT_NAME",
	"SERVER_NAME",
	"SERVER_PORT",
	"SERVER_PROTOCOL",
	"SERVER_SOFTWARE",
];

/// Common-extension variables (not in RFC 3875 but ubiquitous).
/// See spec § _Common extensions, always set_.
const COMMON_EXTENSIONS: &[&str] =
	&["REMOTE_PORT", "REQUEST_URI", "REQUEST_SCHEME", "HTTPS", "DOCUMENT_URI"];

/// Returns true when the user-supplied env key collides with a value
/// vane computes per request. The `HTTP_*` shape catches request-header
/// passthrough vars regardless of which header name produced them.
fn is_reserved_env_key(key: &str) -> bool {
	if key.starts_with("HTTP_") {
		return true;
	}
	RFC_3875_REQUIRED.contains(&key) || COMMON_EXTENSIONS.contains(&key)
}

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
	let parsed = parse_args(args).map_err(FactoryError)?;
	validate_binary(&parsed.binary, &parsed.security).map_err(FactoryError)?;
	if parsed.security.uid == 0 {
		tracing::warn!(
			binary = %parsed.binary.display(),
			"cgi rule configured to run as root; verify this is intended",
		);
	}
	Ok(FetchInst::L7(Arc::new(CgiFetch { args: parsed })))
}

/// Non-unix stub. The CGI driver is unix-only — `pre_exec` is the
/// load-bearing primitive and has no Windows / WASI analogue.
#[cfg(not(unix))]
pub fn factory(_args: &Value) -> Result<FetchInst, FactoryError> {
	Err(FactoryError("CGI fetch driver is unix-only".to_string()))
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
				 cannot override values vane computes per request — see spec § _User-defined, per rule_"
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
	// stable for a future post-MVP implementation pass. Spec § _Security_:
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

/// Bootstrap validation per spec § _Bootstrap validation_: rule-level
/// compile error (not daemon-wide) when the binary is missing /
/// non-file / non-executable for the configured uid.
//
// `similar_names` flags `file_uid` / `file_gid` against each other —
// they're a pair of related fields and the close naming is the clearer
// expression here.
#[allow(clippy::similar_names)]
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

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// Daemon-wide cap on simultaneously running CGI children. Spec §
/// _Concurrency cap_: when reached, new requests fast-reject with 503;
/// no queueing.
///
/// The semaphore is built once per process from
/// `VANE_CGI_MAX_CONCURRENT` (default 100). The `OnceLock` initializer
/// runs lazily on the first CGI request — daemon init does not need
/// to poke the slot.
///
/// `cap` is captured alongside the [`Semaphore`] so `pool_stats()` can
/// report `(cap, available)` consistently — `tokio::sync::Semaphore`
/// itself does not expose its initial permit count, and re-reading
/// `VANE_CGI_MAX_CONCURRENT` would race with operator-side env churn.
struct CgiPermitState {
	semaphore: Arc<Semaphore>,
	cap: usize,
	/// Cumulative successful permit acquisitions — i.e. CGI fetches that
	/// crossed the cap gate and proceeded to fork/exec.
	total_spawns: Arc<std::sync::atomic::AtomicU64>,
	/// Cumulative `try_acquire_owned` failures — fast-rejects under the
	/// concurrency cap (spec § _Concurrency cap_).
	failures: Arc<std::sync::atomic::AtomicU64>,
}

static CGI_PERMITS: OnceLock<CgiPermitState> = OnceLock::new();

const DEFAULT_MAX_CONCURRENT: usize = 100;

/// Per-rule body-size limit for stdin writes. The spec § _Streaming
/// posture: half-buffered_ refers to "`max_body_size` on the request
/// side" without nailing down where the value comes from; the
/// rule-level `max_body_bytes_request` field already carries an
/// 8 MiB default for every rule. Until that field is plumbed through
/// to `L7Fetch` we use the same constant here so behaviour matches.
const CGI_MAX_REQUEST_BODY: usize = 8 * 1024 * 1024;

const TERMINATE_GRACE: Duration = Duration::from_secs(1);

fn cgi_permits() -> Arc<Semaphore> {
	Arc::clone(
		&CGI_PERMITS
			.get_or_init(|| {
				let cap = std::env::var("VANE_CGI_MAX_CONCURRENT")
					.ok()
					.and_then(|s| s.parse::<usize>().ok())
					.filter(|n| *n > 0)
					.unwrap_or(DEFAULT_MAX_CONCURRENT);
				CgiPermitState {
					semaphore: Arc::new(Semaphore::new(cap)),
					cap,
					total_spawns: Arc::new(std::sync::atomic::AtomicU64::new(0)),
					failures: Arc::new(std::sync::atomic::AtomicU64::new(0)),
				}
			})
			.semaphore,
	)
}

/// Counter handles tied to the lazily-initialised permit state. Returns
/// `None` when the state has not yet been touched (no CGI traffic yet).
fn cgi_permit_counters()
-> Option<(Arc<std::sync::atomic::AtomicU64>, Arc<std::sync::atomic::AtomicU64>)> {
	let state = CGI_PERMITS.get()?;
	Some((Arc::clone(&state.total_spawns), Arc::clone(&state.failures)))
}

/// Snapshot of the CGI concurrency cap. Read-only: returns `None`
/// until the semaphore is lazily initialised on the first CGI request.
///
/// The mgmt-verb path must not trigger first-init — operators reading
/// `get_pools` before any CGI traffic should see the absent state, not
/// implicitly bake `VANE_CGI_MAX_CONCURRENT` into a process-wide
/// constant on a cold daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CgiPoolStats {
	pub cap: usize,
	pub available: usize,
	pub in_use: usize,
	/// Cumulative successful permit acquisitions — translated to
	/// `total_allocations` on the wire shape.
	pub total_allocations: u64,
	/// Cumulative cap-rejected acquisitions.
	pub failures: u64,
}

#[must_use]
pub fn pool_stats() -> Option<CgiPoolStats> {
	let state = CGI_PERMITS.get()?;
	let available = state.semaphore.available_permits();
	let in_use = state.cap.saturating_sub(available);
	Some(CgiPoolStats {
		cap: state.cap,
		available,
		in_use,
		total_allocations: state.total_spawns.load(std::sync::atomic::Ordering::Relaxed),
		failures: state.failures.load(std::sync::atomic::Ordering::Relaxed),
	})
}

#[cfg(unix)]
struct CgiFetch {
	args: CgiArgs,
}

#[cfg(unix)]
#[async_trait]
impl L7Fetch for CgiFetch {
	async fn fetch(
		&self,
		req: Request,
		conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// Spec § _Concurrency cap_: fast-reject with 503 when the
		// daemon-wide CGI permit pool is empty. Queueing under
		// sustained overload amplifies pressure (each pending
		// request still holds its connection); surfacing the cap to
		// operators is the spec's explicit choice.
		// Drive `cgi_permits()` first so the OnceLock is initialised
		// before we try to read the counter handles — otherwise the
		// first call would observe `None` and miss its own counter
		// bump.
		let semaphore = cgi_permits();
		let counters = cgi_permit_counters();
		let Ok(permit) = semaphore.try_acquire_owned() else {
			if let Some((_, failures)) = &counters {
				failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
			}
			return Ok(L7FetchOutput::Response(static_response(StatusCode::SERVICE_UNAVAILABLE)));
		};
		if let Some((spawns, _)) = &counters {
			spawns.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
		}

		let total_deadline = Instant::now() + self.args.timeouts.total;
		match tokio::time::timeout_at(total_deadline, self.run(req, conn, permit, total_deadline)).await
		{
			Ok(out) => out,
			Err(_) => Ok(L7FetchOutput::Response(static_response(StatusCode::GATEWAY_TIMEOUT))),
		}
	}
}

#[cfg(unix)]
impl CgiFetch {
	#[allow(clippy::too_many_lines)]
	async fn run(
		&self,
		req: Request,
		conn: &Arc<ConnContext>,
		permit: tokio::sync::OwnedSemaphorePermit,
		total_deadline: Instant,
	) -> Result<L7FetchOutput, Error> {
		// Build the env up-front (per-request). The builder is
		// infallible — every input is already validated at link time
		// or comes from connection state.
		let env = build_env(&self.args, &req, conn);

		let mut cmd = Command::new(&self.args.binary);
		cmd
			.env_clear()
			.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
			.current_dir(&self.args.working_dir)
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped());

		install_pre_exec(&mut cmd, self.args.security.clone());

		let mut child = match cmd.spawn() {
			Ok(c) => c,
			Err(e) => {
				tracing::warn!(
					target: "vane::cgi",
					binary = %self.args.binary.display(),
					error = %e,
					"cgi spawn failed",
				);
				return Ok(L7FetchOutput::Response(static_response(StatusCode::BAD_GATEWAY)));
			}
		};

		let stdin = child.stdin.take().expect("stdin piped");
		let stdout = child.stdout.take().expect("stdout piped");
		let stderr = child.stderr.take().expect("stderr piped");
		let pid = child.id();

		let binary_for_stderr = self.args.binary.clone();
		tokio::spawn(stderr_drain(stderr, binary_for_stderr, pid));

		let body = req.into_body();
		let stdin_task = tokio::spawn(stdin_drain(stdin, body, CGI_MAX_REQUEST_BODY));

		// `read_until_header_end` is the single arbiter of the
		// connect-phase outcome. It produces three possible signals:
		//
		// * `Ok((headers, leftover, stdout))` — header block parsed.
		// * `Err(HeaderEofWithExitCode)` — stdout EOFed before a
		//   `\r\n\r\n` was seen (child crashed without producing a
		//   valid response → 502).
		// * `Err(ConnectTimeout)` — the connect_timeout deadline
		//   fired (504).
		//
		// We deliberately do NOT race a `child.wait()` arm against
		// this future: a fast-exiting child that wrote a valid
		// response can complete `wait()` before the parent's
		// `read()` drains stdout, and treating that as an early
		// exit would override an otherwise-good response with a
		// false-positive 502. The "child exited without headers"
		// case still surfaces — the kernel closes the stdout pipe
		// on `_exit(2)`, the parent's read returns 0, and
		// `read_until_header_end` reports `HeaderEofWithExitCode`.
		let connect_deadline = Instant::now() + self.args.timeouts.connect;
		let parsed = read_until_header_end(stdout, connect_deadline).await;

		let (header_block, leftover, stdout) = match parsed {
			Ok(triple) => triple,
			Err(early) => {
				let status = match early {
					EarlyExit::HeaderEofWithExitCode(code) => {
						tracing::warn!(
							target: "vane::cgi",
							binary = %self.args.binary.display(),
							pid = pid.unwrap_or(0),
							exit = code,
							"cgi child exited before producing a usable header block",
						);
						StatusCode::BAD_GATEWAY
					}
					EarlyExit::ConnectTimeout => {
						terminate_child(&mut child).await;
						StatusCode::GATEWAY_TIMEOUT
					}
				};
				stdin_task.abort();
				let _ = child.wait().await;
				drop(permit);
				return Ok(L7FetchOutput::Response(static_response(status)));
			}
		};

		let resp_builder = match build_response_from_headers(&header_block) {
			Ok(b) => b,
			Err(e) => {
				tracing::warn!(
					target: "vane::cgi",
					binary = %self.args.binary.display(),
					pid = pid.unwrap_or(0),
					error = %e,
					"cgi header parse failed",
				);
				let _ = child.kill().await;
				stdin_task.abort();
				drop(permit);
				return Ok(L7FetchOutput::Response(static_response(StatusCode::BAD_GATEWAY)));
			}
		};

		// Tail task: wait for child + log non-zero exit. Headers are
		// already on the wire, so the exit code is informational from
		// here on; the spec's "non-zero → 502" applies only to the
		// header-block-EOF path above.
		let binary_for_exit = self.args.binary.clone();
		tokio::spawn(async move {
			match child.wait().await {
				Ok(status) if !status.success() => {
					tracing::warn!(
						target: "vane::cgi",
						binary = %binary_for_exit.display(),
						pid = pid.unwrap_or(0),
						exit_code = status.code().unwrap_or(-1),
						"cgi child exited non-zero (after streaming response)",
					);
				}
				Ok(_) => {}
				Err(e) => {
					tracing::warn!(
						target: "vane::cgi",
						binary = %binary_for_exit.display(),
						pid = pid.unwrap_or(0),
						error = %e,
						"cgi child wait() failed",
					);
				}
			}
			drop(stdin_task);
		});

		let body_stream = CgiResponseBody::new(leftover, stdout, total_deadline, permit);
		let response = resp_builder
			.body(Body::Stream(Box::pin(body_stream)))
			.map_err(|e| Error::internal(format!("cgi response build: {e}")))?;
		Ok(L7FetchOutput::Response(response))
	}
}

#[cfg(unix)]
fn static_response(status: StatusCode) -> Response {
	let mut b = http::Response::builder().status(status);
	if status == StatusCode::SERVICE_UNAVAILABLE {
		b = b.header(http::header::CACHE_CONTROL, "no-store");
	}
	b.body(Body::Empty).expect("static response")
}

/// Build the RFC 3875 + common-extension env for one request. Spec §
/// _Required by RFC 3875_ + § _Common extensions, always set_.
#[cfg(unix)]
#[allow(clippy::too_many_lines)]
fn build_env(args: &CgiArgs, req: &Request, conn: &Arc<ConnContext>) -> Vec<(String, String)> {
	let mut env: Vec<(String, String)> = Vec::with_capacity(32);

	let uri = req.uri();
	let path = uri.path();
	let path_info = path.strip_prefix(args.script_name.as_str()).unwrap_or(path);
	let mut path_translated = args.working_dir.clone();
	if !path_info.is_empty() {
		path_translated.push(path_info.trim_start_matches('/'));
	}
	let path_translated = path_translated.to_string_lossy().into_owned();
	let query = uri.query().unwrap_or("");
	let request_uri = uri.path_and_query().map_or_else(|| path.to_owned(), ToString::to_string);

	let method = req.method().as_str();
	let content_length = req
		.headers()
		.get(http::header::CONTENT_LENGTH)
		.and_then(|v| v.to_str().ok())
		.unwrap_or("0")
		.to_owned();
	let content_type = req
		.headers()
		.get(http::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.to_owned();
	let server_name = req
		.headers()
		.get(http::header::HOST)
		.and_then(|v| v.to_str().ok())
		.map_or_else(|| conn.local.ip().to_string(), str::to_owned);

	let is_tls = conn.tls.lock().is_some();

	env.push(("CONTENT_LENGTH".to_owned(), content_length));
	env.push(("CONTENT_TYPE".to_owned(), content_type));
	env.push(("GATEWAY_INTERFACE".to_owned(), "CGI/1.1".to_owned()));
	env.push(("PATH_INFO".to_owned(), path_info.to_owned()));
	env.push(("PATH_TRANSLATED".to_owned(), path_translated));
	env.push(("QUERY_STRING".to_owned(), query.to_owned()));
	env.push(("REMOTE_ADDR".to_owned(), conn.remote.ip().to_string()));
	env.push(("REMOTE_HOST".to_owned(), conn.remote.ip().to_string()));
	env.push(("REQUEST_METHOD".to_owned(), method.to_owned()));
	env.push(("SCRIPT_NAME".to_owned(), args.script_name.clone()));
	env.push(("SERVER_NAME".to_owned(), server_name));
	env.push(("SERVER_PORT".to_owned(), conn.local.port().to_string()));
	env.push(("SERVER_PROTOCOL".to_owned(), "HTTP/1.1".to_owned()));
	env.push(("SERVER_SOFTWARE".to_owned(), concat!("vane/", env!("CARGO_PKG_VERSION")).to_owned()));

	env.push(("REMOTE_PORT".to_owned(), conn.remote.port().to_string()));
	env.push(("REQUEST_URI".to_owned(), request_uri));
	env.push((
		"REQUEST_SCHEME".to_owned(),
		if is_tls { "https".to_owned() } else { "http".to_owned() },
	));
	if is_tls {
		env.push(("HTTPS".to_owned(), "on".to_owned()));
	}
	env.push(("DOCUMENT_URI".to_owned(), path.to_owned()));

	for (name, value) in req.headers() {
		let lower = name.as_str().to_ascii_lowercase();
		if lower == "content-length" || lower == "content-type" {
			continue;
		}
		if args.block_headers.iter().any(|b| b.eq_ignore_ascii_case(name.as_str())) {
			continue;
		}
		let key = format!("HTTP_{}", name.as_str().to_ascii_uppercase().replace('-', "_"));
		let val = value.to_str().unwrap_or("").to_owned();
		env.push((key, val));
	}

	for (k, v) in &args.env {
		env.push((k.clone(), v.clone()));
	}

	env
}

/// Install the `pre_exec` closure that drops privileges + applies
/// rlimits in the child process between fork and exec. See the
/// module-level "unsafe boundary" doc for the safety discipline.
#[cfg(unix)]
fn install_pre_exec(cmd: &mut Command, security: CgiSecurity) {
	// `tokio::process::Command::pre_exec` is its own inherent method
	// (it mirrors `std::os::unix::process::CommandExt::pre_exec` but
	// is not the trait method itself), so no `use` import is needed.
	//
	// SAFETY: see module-level doc § _unsafe boundary_. The closure
	// body is async-signal-safe — only the listed syscalls fire, no
	// heap allocation, no mutex acquisition, no non-listed file I/O.
	// Errors are propagated to the parent via the `io::Error` return
	// value, which `spawn()` surfaces in the returned future.
	unsafe {
		cmd.pre_exec(move || pre_exec_drop_privs_and_limits(&security));
	}
}

/// Body of the `pre_exec` closure, broken out so the unsafe block
/// stays small. Async-signal-safe: no allocation, no mutex, only the
/// listed syscalls.
#[cfg(unix)]
fn pre_exec_drop_privs_and_limits(security: &CgiSecurity) -> io::Result<()> {
	// Order matters: setgid before setuid. Once setuid drops to a
	// non-root uid the process loses CAP_SETGID, so any
	// supplementary-gid changes have to land before the uid drop.
	// SAFETY: setgid / setuid are listed POSIX async-signal-safe
	// syscalls. Both take a primitive `gid_t` / `uid_t` and don't
	// touch the heap.
	if unsafe { libc::setgid(security.gid as libc::gid_t) } != 0 {
		return Err(io::Error::last_os_error());
	}
	if unsafe { libc::setuid(security.uid as libc::uid_t) } != 0 {
		return Err(io::Error::last_os_error());
	}
	apply_rlimit(
		libc::RLIMIT_AS,
		security.limits.memory_mb.map(|mb| mb.saturating_mul(1024 * 1024)),
	)?;
	apply_rlimit(libc::RLIMIT_CPU, security.limits.cpu_seconds)?;
	apply_rlimit(libc::RLIMIT_NPROC, security.limits.max_processes)?;
	Ok(())
}

#[cfg(unix)]
fn apply_rlimit(resource: libc::c_int, limit: Option<u64>) -> io::Result<()> {
	let Some(value) = limit else { return Ok(()) };
	let v = value as libc::rlim_t;
	let rl = libc::rlimit { rlim_cur: v, rlim_max: v };
	// SAFETY: setrlimit is async-signal-safe. The struct is owned in
	// this stack frame, no heap pointers escape; the kernel reads its
	// fields by value. The `as _` cast normalises `c_int` to the
	// platform-specific `setrlimit` first-arg type — Linux gnu uses
	// `__rlimit_resource_t` (a u32), every other unix uses `c_int`.
	if unsafe { libc::setrlimit(resource as _, &raw const rl) } != 0 {
		return Err(io::Error::last_os_error());
	}
	Ok(())
}

#[cfg(unix)]
async fn stdin_drain(
	mut stdin: tokio::process::ChildStdin,
	body: Body,
	limit: usize,
) -> io::Result<()> {
	let mut body = body;
	let mut total: usize = 0;
	loop {
		let frame_opt = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
		let frame = match frame_opt {
			Some(Ok(f)) => f,
			Some(Err(e)) => return Err(io::Error::other(format!("request body: {e}"))),
			None => break,
		};
		if let Ok(data) = frame.into_data() {
			total = total.saturating_add(data.len());
			if total > limit {
				return Err(io::Error::other("request body exceeds CGI per-rule limit"));
			}
			if let Err(e) = stdin.write_all(&data).await {
				if e.kind() == io::ErrorKind::BrokenPipe {
					return Ok(());
				}
				return Err(e);
			}
		}
	}
	stdin.shutdown().await
}

#[cfg(unix)]
async fn stderr_drain(stderr: tokio::process::ChildStderr, binary: PathBuf, pid: Option<u32>) {
	let reader = BufReader::new(stderr);
	let mut lines = reader.lines();
	loop {
		match lines.next_line().await {
			Ok(Some(line)) => {
				tracing::warn!(
					target: "vane::cgi",
					binary = %binary.display(),
					pid = pid.unwrap_or(0),
					message = %line,
				);
			}
			Ok(None) | Err(_) => return,
		}
	}
}

#[cfg(unix)]
enum EarlyExit {
	HeaderEofWithExitCode(i32),
	ConnectTimeout,
}

#[cfg(unix)]
async fn read_until_header_end(
	mut stdout: ChildStdout,
	deadline: Instant,
) -> Result<(Vec<u8>, Vec<u8>, ChildStdout), EarlyExit> {
	let mut buf = Vec::with_capacity(1024);
	let mut tmp = [0u8; 4096];
	loop {
		let read = tokio::time::timeout_at(deadline, stdout.read(&mut tmp))
			.await
			.map_err(|_| EarlyExit::ConnectTimeout)?;
		match read {
			Ok(n) if n > 0 => {
				buf.extend_from_slice(&tmp[..n]);
				if let Some(end) = find_header_end(&buf) {
					let leftover = buf.split_off(end);
					return Ok((buf, leftover, stdout));
				}
			}
			// Either EOF (n == 0) or a read error — both mean the
			// child won't produce any more bytes; map to "no usable
			// header block" so the caller surfaces 502.
			Ok(_) | Err(_) => return Err(EarlyExit::HeaderEofWithExitCode(-1)),
		}
	}
}

#[cfg(unix)]
fn find_header_end(buf: &[u8]) -> Option<usize> {
	buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Build a `http::response::Builder` from an RFC 3875 header block.
/// Spec § _stdin / stdout protocol_:
///
/// * `Status: 200 OK` sets the status code (CGI-specific header,
///   not an HTTP/1.1 status line).
/// * `Location: /...` without a `Status:` produces 302.
/// * Other headers pass through.
#[cfg(unix)]
fn build_response_from_headers(block: &[u8]) -> Result<http::response::Builder, String> {
	let s = std::str::from_utf8(block).map_err(|e| format!("non-utf8 header block: {e}"))?;
	let mut status: Option<StatusCode> = None;
	let mut location_seen = false;
	let mut builder = http::Response::builder();
	for line in s.split("\r\n") {
		if line.is_empty() {
			continue;
		}
		let (name, value) =
			line.split_once(':').ok_or_else(|| format!("malformed header: {line:?}"))?;
		let name = name.trim();
		let value = value.trim();
		if name.eq_ignore_ascii_case("Status") {
			let code =
				value.split_whitespace().next().ok_or_else(|| format!("Status header empty: {value:?}"))?;
			let parsed = code.parse::<u16>().map_err(|e| format!("Status code parse: {e}"))?;
			status = Some(StatusCode::from_u16(parsed).map_err(|e| format!("Status code invalid: {e}"))?);
		} else {
			let header_name =
				HeaderName::try_from(name).map_err(|e| format!("invalid header name {name:?}: {e}"))?;
			let header_val = HeaderValue::try_from(value)
				.map_err(|e| format!("invalid header value for {name:?}: {e}"))?;
			if header_name.as_str().eq_ignore_ascii_case("location") {
				location_seen = true;
			}
			builder = builder.header(header_name, header_val);
		}
	}
	let final_status = match (status, location_seen) {
		(Some(s), _) => s,
		(None, true) => StatusCode::FOUND,
		(None, false) => StatusCode::OK,
	};
	Ok(builder.status(final_status))
}

/// Send `SIGTERM`, wait up to one second, then `SIGKILL`. Used for
/// timeout-driven termination per spec § _Timeouts_.
#[cfg(unix)]
async fn terminate_child(child: &mut tokio::process::Child) {
	if let Some(pid) = child.id() {
		// `pid_t` is signed by POSIX; `child.id()` returns it as
		// `Option<u32>` with PID 0 reserved (never returned). The
		// reinterpret-cast is bit-equivalent for any real PID and
		// surfaces in the kernel as the correct signed value.
		let pid_signed: libc::pid_t = pid.cast_signed();
		// SAFETY: kill(2) is async-signal-safe. The pid was obtained
		// from `child.id()` which holds an OS-level reference for
		// the lifetime of the `Child`.
		unsafe {
			libc::kill(pid_signed, libc::SIGTERM);
		}
	}
	let _ = tokio::time::timeout(TERMINATE_GRACE, child.wait()).await;
	let _ = child.start_kill();
	let _ = child.wait().await;
}

/// Streaming response body: yields the bytes that arrived in the
/// header-drain buffer first, then reads the rest from the child's
/// stdout. Holds the daemon-wide CGI permit until the body is fully
/// consumed (so the cap actually reflects in-flight CGI children,
/// not just spawn-throughput).
#[cfg(unix)]
struct CgiResponseBody {
	initial: Option<Bytes>,
	stdout: ChildStdout,
	deadline: Instant,
	_permit: tokio::sync::OwnedSemaphorePermit,
}

#[cfg(unix)]
impl CgiResponseBody {
	fn new(
		initial: Vec<u8>,
		stdout: ChildStdout,
		deadline: Instant,
		permit: tokio::sync::OwnedSemaphorePermit,
	) -> Self {
		let initial = if initial.is_empty() { None } else { Some(Bytes::from(initial)) };
		Self { initial, stdout, deadline, _permit: permit }
	}
}

#[cfg(unix)]
impl HttpBody for CgiResponseBody {
	type Data = Bytes;
	type Error = Error;

	fn poll_frame(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		if let Some(b) = self.initial.take() {
			return Poll::Ready(Some(Ok(Frame::data(b))));
		}
		if Instant::now() >= self.deadline {
			return Poll::Ready(Some(Err(Error::io("cgi total_timeout exceeded mid-body"))));
		}
		let mut buf = [0u8; 4096];
		let mut read_buf = ReadBuf::new(&mut buf);
		match Pin::new(&mut self.stdout).poll_read(cx, &mut read_buf) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(Ok(())) => {
				let filled = read_buf.filled();
				if filled.is_empty() {
					Poll::Ready(None)
				} else {
					Poll::Ready(Some(Ok(Frame::data(Bytes::copy_from_slice(filled)))))
				}
			}
			Poll::Ready(Err(e)) => Poll::Ready(Some(Err(Error::io(format!("cgi stdout read: {e}"))))),
		}
	}

	fn is_end_stream(&self) -> bool {
		false
	}

	fn size_hint(&self) -> SizeHint {
		SizeHint::default()
	}
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
		assert!(err.0.contains("binary"), "error must name the missing field: {err:?}");
	}

	#[test]
	fn factory_rejects_missing_script_name_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("script_name");
		let err = expect_factory_err(&args);
		assert!(err.0.contains("script_name"), "must name field: {err:?}");
	}

	#[test]
	fn factory_rejects_missing_working_dir_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("working_dir");
		let err = expect_factory_err(&args);
		assert!(err.0.contains("working_dir"), "must name field: {err:?}");
	}

	#[test]
	fn factory_rejects_missing_env_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("env");
		let err = expect_factory_err(&args);
		assert!(err.0.contains("env"), "must name field: {err:?}");
	}

	#[test]
	fn factory_rejects_missing_block_headers_field() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args.as_object_mut().unwrap().remove("block_headers");
		let err = expect_factory_err(&args);
		assert!(err.0.contains("block_headers"), "must name field: {err:?}");
	}

	#[test]
	fn factory_rejects_chroot_some_with_spec_text() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["security"]["chroot"] = json!("/var/empty");
		let err = expect_factory_err(&args);
		assert!(
			err.0.contains("chroot is reserved but not yet implemented"),
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
		assert!(err.0.contains("memory_mb"), "must name field: {err:?}");
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
		assert!(err.0.contains("REQUEST_METHOD"), "must name the offending key: {err:?}");
		assert!(err.0.contains("reserved"), "must explain why: {err:?}");
	}

	#[test]
	fn factory_rejects_env_with_reserved_http_prefixed_key() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["env"] = json!({ "HTTP_USER_AGENT": "x" });
		let err = expect_factory_err(&args);
		assert!(err.0.contains("HTTP_USER_AGENT"), "must name the offending key: {err:?}");
	}

	#[test]
	fn factory_rejects_env_with_reserved_common_extension_key() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["env"] = json!({ "HTTPS": "on" });
		let err = expect_factory_err(&args);
		assert!(err.0.contains("HTTPS"), "must name the offending key: {err:?}");
	}

	#[test]
	fn factory_rejects_binary_that_does_not_exist() {
		let bin = fixture_binary();
		let mut args = minimal_valid_args(bin.path());
		args["binary"] = json!("/no/such/path/here-cgi-fixture");
		let err = expect_factory_err(&args);
		assert!(err.0.contains("not accessible"), "must explain: {err:?}");
	}

	#[test]
	fn factory_rejects_binary_that_is_a_directory() {
		let bin = fixture_binary();
		let dir = bin.path().parent().unwrap();
		let mut args = minimal_valid_args(bin.path());
		args["binary"] = json!(dir.to_str().unwrap());
		let err = expect_factory_err(&args);
		assert!(err.0.contains("not a regular file"), "must explain: {err:?}");
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
		assert!(err.0.contains("not executable"), "must explain: {err:?}");
	}

	#[test]
	fn is_reserved_env_key_recognises_each_set() {
		assert!(is_reserved_env_key("REQUEST_METHOD"));
		assert!(is_reserved_env_key("HTTPS"));
		assert!(is_reserved_env_key("HTTP_USER_AGENT"));
		assert!(!is_reserved_env_key("DATABASE_URL"));
		assert!(!is_reserved_env_key("APP_MODE"));
	}

	#[test]
	fn pool_stats_reports_state_after_first_init() {
		// Drive the lazy init exactly once via the same code path that
		// CgiFetch::fetch uses. Once the semaphore is live, pool_stats
		// must report a fully-available pool (no permits held).
		//
		// Cannot assert the pre-init `None` shape here because other
		// unit tests in the crate's test binary may have already fired
		// the OnceLock; the dispatcher / e2e tests cover that arm.
		let _ = cgi_permits();
		let stats = pool_stats().expect("semaphore initialised");
		assert!(stats.cap > 0);
		assert_eq!(stats.available, stats.cap, "no in-flight CGI children in this test binary");
		assert_eq!(stats.in_use, 0);
	}
}
