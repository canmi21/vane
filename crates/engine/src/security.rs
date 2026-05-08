//! L1 security floor — daemon self-preservation: per-IP + global
//! connection caps, header / body timeouts, floor-enforcement at
//! startup.
//!
//! State is daemon-scoped (lives outside `FlowGraph`), so config reload
//! does not reset counters. See `spec/crates/core.md` §
//! _L1 — Daemon self-preservation_.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio_util::sync::CancellationToken;
use vane_core::{Error, config::Env};

use crate::tls::CrlCache;

// Spec-defined minimums from spec/crates/core.md.
const FLOOR_HEADER_TIMEOUT_SECS: u32 = 5;
const FLOOR_MAX_HEADER_BYTES: usize = 4_096;
const FLOOR_MAX_HEADERS_COUNT: usize = 20;
const FLOOR_MAX_CONN_PER_IP: usize = 10;
const FLOOR_MAX_TOTAL_CONNS: usize = 1_024;

/// Typed snapshot of `VANE_SEC_*` deployment constants. Values are
/// validated against spec-defined minimums at construction time;
/// below-floor values are a startup error (`Error::compile`).
///
/// `Default` uses spec-defined defaults for test code that does not
/// need to exercise floor enforcement.
#[derive(Clone)]
pub struct SecurityConfig {
	/// `VANE_SEC_HEADER_TIMEOUT` — wall-clock budget from TCP accept
	/// to complete HTTP headers (default 30 s, floor 5 s). Applied to
	/// the L4 peek phase and hyper's `header_read_timeout`.
	pub header_timeout: Duration,
	/// `VANE_SEC_MAX_HEADER_BYTES` — maximum parsed header bytes per
	/// request (default 65536, floor 4096). The H1 read buffer is set
	/// to 4× this value to leave room for body chunking; the
	/// service-fn enforces this limit precisely on parsed header
	/// fields.
	pub max_header_bytes: usize,
	/// `VANE_SEC_MAX_HEADERS_COUNT` — maximum number of header fields
	/// per request (default 100, floor 20).
	pub max_headers_count: usize,
	/// `VANE_SEC_MAX_CONN_PER_IP` — maximum concurrent connections
	/// from a single source IP (default 100, floor 10). Soft cap:
	/// minor overcount bounded by tokio worker count is acceptable.
	pub max_conn_per_ip: usize,
	/// `VANE_SEC_MAX_TOTAL_CONNS` — daemon-wide maximum concurrent
	/// connections (default 65536, floor 1024).
	pub max_total_conns: usize,
	/// Daemon-wide CRL cache shared by listener mTLS and upstream
	/// verification. `None` for tests / default builds without CRL
	/// support; populated by daemon main when at least one rule
	/// references a CRL source. See `spec/crates/engine-tls.md`
	/// § _CRL_ § _CRL_.
	pub crl_cache: Option<Arc<CrlCache>>,
}

impl std::fmt::Debug for SecurityConfig {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("SecurityConfig")
			.field("header_timeout", &self.header_timeout)
			.field("max_header_bytes", &self.max_header_bytes)
			.field("max_headers_count", &self.max_headers_count)
			.field("max_conn_per_ip", &self.max_conn_per_ip)
			.field("max_total_conns", &self.max_total_conns)
			.field("crl_cache", &self.crl_cache.is_some())
			.finish()
	}
}

impl Default for SecurityConfig {
	fn default() -> Self {
		Self {
			header_timeout: Duration::from_secs(30),
			max_header_bytes: 65_536,
			max_headers_count: 100,
			max_conn_per_ip: 100,
			max_total_conns: 65_536,
			crl_cache: None,
		}
	}
}

impl SecurityConfig {
	/// Build from the daemon's `Env`. Validates each `VANE_SEC_*`
	/// value against the spec floor; any below-floor value returns
	/// [`Error::compile`] naming the variable and minimum.
	///
	/// # Errors
	/// [`Error::compile`] when any value is below its floor.
	pub fn new(env: &Env) -> Result<Self, Error> {
		floor_u32(env.sec_header_timeout_secs, "VANE_SEC_HEADER_TIMEOUT", FLOOR_HEADER_TIMEOUT_SECS)?;
		floor_usize(
			env.sec_max_header_bytes as usize,
			"VANE_SEC_MAX_HEADER_BYTES",
			FLOOR_MAX_HEADER_BYTES,
		)?;
		floor_usize(
			env.sec_max_headers_count as usize,
			"VANE_SEC_MAX_HEADERS_COUNT",
			FLOOR_MAX_HEADERS_COUNT,
		)?;
		floor_usize(
			env.sec_max_conn_per_ip as usize,
			"VANE_SEC_MAX_CONN_PER_IP",
			FLOOR_MAX_CONN_PER_IP,
		)?;
		floor_usize(
			env.sec_max_total_conns as usize,
			"VANE_SEC_MAX_TOTAL_CONNS",
			FLOOR_MAX_TOTAL_CONNS,
		)?;
		Ok(Self {
			header_timeout: Duration::from_secs(env.sec_header_timeout_secs.into()),
			max_header_bytes: env.sec_max_header_bytes as usize,
			max_headers_count: env.sec_max_headers_count as usize,
			max_conn_per_ip: env.sec_max_conn_per_ip as usize,
			max_total_conns: env.sec_max_total_conns as usize,
			crl_cache: None,
		})
	}
}

fn floor_u32(val: u32, var: &str, floor: u32) -> Result<(), Error> {
	if val < floor {
		Err(Error::compile(format!(
			"{var} = {val} is below the required minimum {floor}; \
			 raise it to at least {floor} or remove the env override"
		)))
	} else {
		Ok(())
	}
}

fn floor_usize(val: usize, var: &str, floor: usize) -> Result<(), Error> {
	if val < floor {
		Err(Error::compile(format!(
			"{var} = {val} is below the required minimum {floor}; \
			 raise it to at least {floor} or remove the env override"
		)))
	} else {
		Ok(())
	}
}

/// Key for log-warning deduplication. One suppression slot per
/// `(limit kind, source IP)` so a flood from a single IP emits at
/// most one warning per second per limit type.
#[derive(Clone, Hash, Eq, PartialEq)]
enum LimitLogKey {
	TotalConns(IpAddr),
	PerIpConns(IpAddr),
}

/// Daemon-scoped L1 security state. Lives outside `FlowGraph` so
/// hot-reload does not reset per-IP or global connection counters.
pub struct SecurityState {
	/// Floor-validated security configuration (read-only after init).
	pub cfg: SecurityConfig,
	per_ip: DashMap<IpAddr, AtomicUsize>,
	total: AtomicUsize,
	/// Last warn timestamp per `(limit, ip)` for 1-second dedup.
	last_warn: DashMap<LimitLogKey, Instant>,
}

impl SecurityState {
	#[must_use]
	pub fn new(cfg: SecurityConfig) -> Self {
		Self { cfg, per_ip: DashMap::new(), total: AtomicUsize::new(0), last_warn: DashMap::new() }
	}

	/// Attempt to register a new connection from `ip`.
	///
	/// Returns a [`ConnSecGuard`] on success — its `Drop` impl
	/// decrements both the global and per-IP counters when the
	/// connection ends, regardless of exit path.
	///
	/// Returns `None` when either cap is exceeded. The caller drops
	/// the TCP stream, which sends RST to the client.
	pub fn check_and_register(self: &Arc<Self>, ip: IpAddr) -> Option<ConnSecGuard> {
		// Global cap first.
		let prev_total = self.total.fetch_add(1, Ordering::AcqRel);
		if prev_total >= self.cfg.max_total_conns {
			self.total.fetch_sub(1, Ordering::Release);
			self.maybe_warn(LimitLogKey::TotalConns(ip), ip, "max_total_conns");
			return None;
		}

		// Per-IP cap.
		let entry = self.per_ip.entry(ip).or_insert_with(|| AtomicUsize::new(0));
		let prev_ip = entry.fetch_add(1, Ordering::AcqRel);
		drop(entry); // release DashMap shard before any await point
		if prev_ip >= self.cfg.max_conn_per_ip {
			if let Some(c) = self.per_ip.get(&ip) {
				c.fetch_sub(1, Ordering::Release);
			}
			self.total.fetch_sub(1, Ordering::Release);
			self.maybe_warn(LimitLogKey::PerIpConns(ip), ip, "max_conn_per_ip");
			return None;
		}

		Some(ConnSecGuard { state: Arc::clone(self), ip })
	}

	fn maybe_warn(&self, key: LimitLogKey, ip: IpAddr, limit: &'static str) {
		let now = Instant::now();
		let emit = match self.last_warn.get(&key) {
			None => true,
			Some(ref t) => now.checked_duration_since(**t).is_none_or(|d| d >= Duration::from_secs(1)),
		};
		if emit {
			self.last_warn.insert(key, now);
			tracing::warn!(%ip, limit, "L1 security limit exceeded — new connection rejected");
		}
	}

	/// Spawn a background task that prunes zero-count per-IP entries
	/// and stale log-dedup slots every 60 seconds. Cancelled via the
	/// supplied token (typically the daemon's shutdown trigger).
	pub fn spawn_cleanup(self: Arc<Self>, cancel: CancellationToken) {
		tokio::spawn(async move {
			loop {
				tokio::select! {
					biased;
					() = cancel.cancelled() => return,
					() = tokio::time::sleep(Duration::from_mins(1)) => {}
				}
				self.per_ip.retain(|_, v| v.load(Ordering::Relaxed) > 0);
				let now = Instant::now();
				self
					.last_warn
					.retain(|_, v| now.checked_duration_since(*v).is_none_or(|d| d < Duration::from_mins(1)));
			}
		});
	}
}

/// RAII guard: decrements global and per-IP connection counters on
/// drop. Held for the duration of `handle_connection` so the
/// decrement runs on every exit path including panics and
/// cancellations.
pub struct ConnSecGuard {
	state: Arc<SecurityState>,
	ip: IpAddr,
}

impl Drop for ConnSecGuard {
	fn drop(&mut self) {
		self.state.total.fetch_sub(1, Ordering::Relaxed);
		if let Some(c) = self.state.per_ip.get(&self.ip) {
			c.fetch_sub(1, Ordering::Relaxed);
		}
	}
}
