//! Child-process plumbing: build the `Command`, install the
//! `pre_exec` privilege-drop + rlimit closure, spawn, and the
//! `SIGTERM` → grace → `SIGKILL` termination helper. The single
//! `unsafe` block in this workspace lives in [`install_pre_exec`] /
//! [`pre_exec_drop_privs_and_limits`] — see the module-level
//! "unsafe boundary" doc on [`super`] for the audit discipline.

#![cfg(unix)]

use std::io;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use tokio::process::Command;
use vane_core::{ConnContext, L7FetchOutput, Request};

use super::CgiSecurity;
use super::runtime::{build_env, static_response};

pub(super) const TERMINATE_GRACE: Duration = Duration::from_secs(1);

/// Pipes + handles produced by [`spawn_cgi_child`].
pub(super) struct Spawned {
	pub child: tokio::process::Child,
	pub stdin: tokio::process::ChildStdin,
	pub stdout: tokio::process::ChildStdout,
	pub stderr: tokio::process::ChildStderr,
	pub pid: Option<u32>,
}

/// Build the env, configure the [`Command`], install the pre-fork
/// privilege-drop hook, spawn the child, and take the piped
/// stdin/stdout/stderr handles. Spawn failure is warn-logged and
/// surfaced as a 502 [`L7FetchOutput`] so the caller can short-circuit.
#[allow(
	clippy::result_large_err,
	reason = "Err carries the early-return response; boxing it would add an allocation per spawn-failure path for no benefit"
)]
pub(super) fn spawn_cgi_child(
	args: &super::CgiArgs,
	req: &Request,
	conn: &Arc<ConnContext>,
) -> Result<Spawned, L7FetchOutput> {
	let env = build_env(args, req, conn);
	let mut cmd = Command::new(&args.binary);
	cmd
		.env_clear()
		.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
		.current_dir(&args.working_dir)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped());
	install_pre_exec(&mut cmd, args.security.clone());
	let mut child = match cmd.spawn() {
		Ok(c) => c,
		Err(e) => {
			tracing::warn!(
				target: "vane::cgi",
				binary = %args.binary.display(),
				error = %e,
				"cgi spawn failed",
			);
			return Err(L7FetchOutput::Response(static_response(StatusCode::BAD_GATEWAY)));
		}
	};
	let stdin = child.stdin.take().expect("stdin piped");
	let stdout = child.stdout.take().expect("stdout piped");
	let stderr = child.stderr.take().expect("stderr piped");
	let pid = child.id();
	Ok(Spawned { child, stdin, stdout, stderr, pid })
}

fn install_pre_exec(cmd: &mut Command, security: CgiSecurity) {
	// `tokio::process::Command::pre_exec` is its own inherent method
	// (it mirrors `std::os::unix::process::CommandExt::pre_exec` but
	// is not the trait method itself), so no `use` import is needed.
	//
	// SAFETY: see module-level doc; `spec/crates/engine.md` § _CGI_. The closure
	// body is async-signal-safe — only the listed syscalls fire, no
	// heap allocation, no mutex acquisition, no non-listed file I/O.
	// Errors are propagated to the parent via the `io::Error` return
	// value, which `spawn()` surfaces in the returned future.
	unsafe {
		cmd.pre_exec(move || pre_exec_drop_privs_and_limits(&security));
	}
}

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

// `libc::RLIMIT_*` and `setrlimit`'s first argument are typed as
// `__rlimit_resource_t` (u32) on linux glibc and `c_int` (i32)
// everywhere else; `apply_rlimit` takes the platform's native type
// so callsites and the `setrlimit` invocation stay cast-free.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
type RlimitResource = libc::c_int;

fn apply_rlimit(resource: RlimitResource, limit: Option<u64>) -> io::Result<()> {
	let Some(value) = limit else { return Ok(()) };
	let v = value as libc::rlim_t;
	let rl = libc::rlimit { rlim_cur: v, rlim_max: v };
	// SAFETY: setrlimit is async-signal-safe. The struct is owned in
	// this stack frame, no heap pointers escape; the kernel reads its
	// fields by value.
	if unsafe { libc::setrlimit(resource, &raw const rl) } != 0 {
		return Err(io::Error::last_os_error());
	}
	Ok(())
}

/// Send `SIGTERM`, wait up to one second, then `SIGKILL`. Used for
/// timeout-driven termination per `spec/crates/engine.md` `spec/crates/engine.md` § _Concrete fetches_.
pub(super) async fn terminate_child(child: &mut tokio::process::Child) {
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
