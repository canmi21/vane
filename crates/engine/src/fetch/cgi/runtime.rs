//! Per-request runtime: `CgiFetch`, the `L7Fetch` impl, header read +
//! parse, stdin/stderr pumps, and the tail-wait task that joins the
//! child after headers are on the wire. Build-time construction lives
//! in [`super::parse`]; the fork+exec primitive lives in
//! [`super::spawn`].

#![cfg(unix)]

use std::future::poll_fn;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use cgi_request::CgiRequestMeta;
use cgi_response::HeaderReadError;
use http::StatusCode;
use http_body::Body as _;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::time::Instant;
use vane_core::{Body, ConnContext, Error, FlowCtx, L7Fetch, L7FetchOutput, Request, Response};

use super::CgiArgs;
use super::pool::{cgi_permit_counters, cgi_permits};
use super::spawn::{Spawned, spawn_cgi_child, terminate_child};

/// Per-rule body-size limit for stdin writes. `spec/crates/engine.md`
/// § _CGI_ refers to "`max_body_size` on the request side" without
/// nailing down where the value comes from; the rule-level
/// `max_body_bytes_request` field already carries an 8 MiB default
/// for every rule. Until that field is plumbed through to `L7Fetch`
/// we use the same constant here so behaviour matches.
const CGI_MAX_REQUEST_BODY: usize = 8 * 1024 * 1024;

pub(super) struct CgiFetch {
	pub(super) args: CgiArgs,
}

#[async_trait]
impl L7Fetch for CgiFetch {
	async fn fetch(
		&self,
		req: Request,
		conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<L7FetchOutput, Error> {
		// `spec/crates/engine.md` § _Concurrency cap_: fast-reject with 503 when the
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

impl CgiFetch {
	async fn run(
		&self,
		req: Request,
		conn: &Arc<ConnContext>,
		permit: tokio::sync::OwnedSemaphorePermit,
		total_deadline: Instant,
	) -> Result<L7FetchOutput, Error> {
		let Spawned { mut child, stdin, stdout, stderr, pid } =
			match spawn_cgi_child(&self.args, &req, conn) {
				Ok(s) => s,
				Err(early) => return Ok(early),
			};

		let binary_for_stderr = self.args.binary.clone();
		tokio::spawn(stderr_drain(stderr, binary_for_stderr, pid));

		let body = req.into_body();
		let stdin_task = tokio::spawn(stdin_drain(stdin, body, CGI_MAX_REQUEST_BODY));

		// `cgi_response::read_until_header_end` is the single arbiter of
		// the connect-phase outcome. It produces three possible signals:
		//
		// * `Ok((headers, leftover, stdout))` — header block parsed.
		// * `Err(HeaderReadError::Eof)` — stdout EOFed before a
		//   `\r\n\r\n` was seen (child crashed without producing a
		//   valid response → 502).
		// * `Err(HeaderReadError::Timeout)` — the connect_timeout
		//   deadline fired (504).
		//
		// We deliberately do NOT race a `child.wait()` arm against
		// this future: a fast-exiting child that wrote a valid
		// response can complete `wait()` before the parent's
		// `read()` drains stdout, and treating that as an early
		// exit would override an otherwise-good response with a
		// false-positive 502. The "child exited without headers"
		// case still surfaces — the kernel closes the stdout pipe
		// on `_exit(2)`, the parent's read returns 0, and the lib
		// reports `Eof`.
		let connect_deadline = Instant::now() + self.args.timeouts.connect;
		let acquired = match read_and_parse_cgi_headers(
			&self.args,
			&mut child,
			pid,
			stdout,
			&stdin_task,
			connect_deadline,
			permit,
		)
		.await
		{
			Ok(a) => a,
			Err(early) => return Ok(early),
		};
		let CgiHeadersAcquired { builder, leftover, stdout, permit } = acquired;

		spawn_cgi_tail_wait(child, pid, stdin_task, self.args.binary.clone());

		// The lib's `CgiResponseBody` carries the permit as a generic
		// drop guard so the daemon-wide concurrency cap continues to
		// reflect in-flight CGI children, not just spawn throughput.
		let body_stream = cgi_response::CgiResponseBody::new(leftover, stdout, total_deadline, permit);
		let response = builder
			.body(Body::from_producer(body_stream))
			.map_err(|e| Error::internal(format!("cgi response build: {e}")))?;
		Ok(L7FetchOutput::Response(response))
	}
}

/// State produced by a successful header-read + parse phase: the
/// `http::response::Builder` ready for `.body(...)`, the leftover bytes
/// the lib consumed past `\r\n\r\n` (replayed into the body stream),
/// the still-open `stdout` for streaming, and the permit echoed back
/// for the body's drop guard.
struct CgiHeadersAcquired {
	builder: http::response::Builder,
	leftover: Vec<u8>,
	stdout: tokio::process::ChildStdout,
	permit: tokio::sync::OwnedSemaphorePermit,
}

/// Connect-phase: read until `\r\n\r\n`, then parse the header block.
///
/// `cgi_response::read_until_header_end` is the single arbiter of the
/// connect outcome:
///
/// * `Ok((headers, leftover, stdout))` — header block parsed.
/// * `Err(HeaderReadError::Eof)` — stdout EOFed before the separator
///   (child crashed without producing a valid response → 502).
/// * `Err(HeaderReadError::Timeout)` — `connect_deadline` fired (504).
///
/// We deliberately do NOT race a `child.wait()` arm against this
/// future: a fast-exiting child that wrote a valid response can
/// complete `wait()` before the parent drains stdout, and treating that
/// as an early exit would override a good response with a false-positive
/// 502. The "child exited without headers" case still surfaces — the
/// kernel closes the stdout pipe on `_exit(2)`, the parent's read
/// returns 0, and the lib reports `Eof`.
///
/// Errors short-circuit through the cleanup choreography (kill / wait
/// child, abort the stdin task, drop the permit) and surface as the
/// matching static response.
#[allow(
	clippy::too_many_arguments,
	reason = "phase aggregator: each param is one in-flight subprocess handle that the cleanup choreography on early-return needs to touch"
)]
#[allow(
	clippy::result_large_err,
	reason = "Err carries the early-return response; boxing it would add an allocation per error path for no benefit"
)]
async fn read_and_parse_cgi_headers(
	args: &CgiArgs,
	child: &mut tokio::process::Child,
	pid: Option<u32>,
	stdout: tokio::process::ChildStdout,
	stdin_task: &tokio::task::JoinHandle<io::Result<()>>,
	connect_deadline: Instant,
	permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<CgiHeadersAcquired, L7FetchOutput> {
	let (header_block, leftover, stdout) =
		match cgi_response::read_until_header_end(stdout, connect_deadline).await {
			Ok(triple) => triple,
			Err(early) => {
				let status = match early {
					HeaderReadError::Eof => {
						tracing::warn!(
							target: "vane::cgi",
							binary = %args.binary.display(),
							pid = pid.unwrap_or(0),
							"cgi child exited before producing a usable header block",
						);
						StatusCode::BAD_GATEWAY
					}
					HeaderReadError::Timeout => {
						terminate_child(child).await;
						StatusCode::GATEWAY_TIMEOUT
					}
				};
				stdin_task.abort();
				let _ = child.wait().await;
				drop(permit);
				return Err(L7FetchOutput::Response(static_response(status)));
			}
		};
	let builder = match cgi_response::parse_response_headers(&header_block) {
		Ok(b) => b,
		Err(e) => {
			tracing::warn!(
				target: "vane::cgi",
				binary = %args.binary.display(),
				pid = pid.unwrap_or(0),
				error = %e,
				"cgi header parse failed",
			);
			let _ = child.kill().await;
			stdin_task.abort();
			drop(permit);
			return Err(L7FetchOutput::Response(static_response(StatusCode::BAD_GATEWAY)));
		}
	};
	Ok(CgiHeadersAcquired { builder, leftover, stdout, permit })
}

/// Spawn the post-headers tail task: wait for the child, log non-zero
/// exit informationally (headers are already on the wire, so the
/// spec's "non-zero → 502" rule no longer applies), and drop the
/// stdin task handle so its drain ends cleanly.
fn spawn_cgi_tail_wait(
	mut child: tokio::process::Child,
	pid: Option<u32>,
	stdin_task: tokio::task::JoinHandle<io::Result<()>>,
	binary: PathBuf,
) {
	tokio::spawn(async move {
		match child.wait().await {
			Ok(status) if !status.success() => {
				tracing::warn!(
					target: "vane::cgi",
					binary = %binary.display(),
					pid = pid.unwrap_or(0),
					exit_code = status.code().unwrap_or(-1),
					"cgi child exited non-zero (after streaming response)",
				);
			}
			Ok(_) => {}
			Err(e) => {
				tracing::warn!(
					target: "vane::cgi",
					binary = %binary.display(),
					pid = pid.unwrap_or(0),
					error = %e,
					"cgi child wait() failed",
				);
			}
		}
		drop(stdin_task);
	});
}

pub(super) fn static_response(status: StatusCode) -> Response {
	let mut b = http::Response::builder().status(status);
	if status == StatusCode::SERVICE_UNAVAILABLE {
		b = b.header(http::header::CACHE_CONTROL, "no-store");
	}
	b.body(Body::Empty).expect("static response")
}

/// Build the RFC 3875 + common-extension env for one request.
/// Thin adapter over [`cgi_request::build_env`]: maps vane's
/// `Request` + `ConnContext` into the lib's [`CgiRequestMeta`]
/// shape. `spec/crates/engine.md` § _CGI_.
pub(super) fn build_env(
	args: &CgiArgs,
	req: &Request,
	conn: &Arc<ConnContext>,
) -> Vec<(String, String)> {
	cgi_request::build_env(&CgiRequestMeta {
		method: req.method().as_str(),
		path: req.uri().path(),
		query: req.uri().query(),
		headers: req.headers(),
		script_name: &args.script_name,
		working_dir: &args.working_dir,
		server_addr: conn.local,
		remote_addr: conn.remote,
		is_tls: conn.tls.lock().is_some(),
		server_software: concat!("vane/", env!("CARGO_PKG_VERSION")),
		block_headers: &args.block_headers,
		extra_env: &args.env,
	})
}

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

// `CgiResponseBody` (with daemon permit as drop guard) lives in the
// `cgi-response` crate; this module only constructs it via the lib.
