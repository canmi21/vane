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
//! The closure in this module (in `spawn`) is the **only** `unsafe`
//! block in the workspace (the workspace-level lint
//! `unsafe_code = "deny"` is lifted here with `#[allow(unsafe_code)]`).
//! The closure is held to the following discipline, audited line by
//! line:
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
//!
//! ## Module layout
//!
//! - `parse` — args parsing, validation, and the public `factory`
//!   entry point. The test suite for factory invariants lives here.
//! - `pool` — daemon-wide concurrency cap (semaphore) and the
//!   `pool_stats` mgmt-verb shape.
//! - `spawn` — `spawn_cgi_child`, the `pre_exec` privilege-drop +
//!   rlimit closure, and the `SIGTERM`/`SIGKILL` termination helper.
//!   This is where the workspace's only `unsafe` block lives.
//! - `runtime` — `CgiFetch`, `impl L7Fetch`, the per-request
//!   header-read / body-pump / stderr-drain pipeline.

#![allow(unsafe_code)] // CGI fork+exec via std::os::unix::process::CommandExt::pre_exec; audited per module doc.

use std::path::PathBuf;
use std::time::Duration;

mod parse;
mod pool;
#[cfg(unix)]
mod runtime;
#[cfg(unix)]
mod spawn;

pub use parse::factory;
pub use pool::{CgiPoolStats, pool_stats};

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

pub(super) const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_mins(1);

// RFC 3875 / common-extension reserved env keys come from the
// `cgi-request` lib's `is_reserved_env_key`. The lib's lists match
// the names this driver computes per request; operators cannot
// override them via `args.env`. See `spec/crates/engine.md` § _CGI_.
