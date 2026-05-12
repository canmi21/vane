//! Retry tokio's `TcpListener` / `UdpSocket` bind against a transient
//! kernel-side bind failure with bounded exponential backoff, while
//! honouring a [`tokio_util::sync::CancellationToken`] so a daemon
//! shutdown does not hang in the retry loop.
//!
//! See the README for the motivating use cases (graceful restart,
//! port-handover dances, lingering `TIME_WAIT` sockets).

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::{TcpListener, TcpSocket, UdpSocket};
use tokio_util::sync::CancellationToken;

/// Retry parameters shared by `tcp` and `udp`.
#[derive(Clone, Debug)]
pub struct Policy {
	/// Total number of bind attempts before giving up. Each failure
	/// counts toward this cap regardless of where it happens (socket
	/// creation, `bind`, or `listen`).
	pub max_attempts: u32,
	/// Initial backoff delay before the second attempt.
	pub initial: Duration,
	/// Cap for the doubled backoff delay.
	pub max: Duration,
	/// Jitter ratio in `[0.0, 1.0]`. The effective delay is
	/// `base * (1 + uniform(-jitter/2, +jitter/2))`, clamped to
	/// `[initial, max]`. Default `0.2` (±10% per attempt) — keeps
	/// many concurrent re-binders (e.g. several listener tasks
	/// racing on the same `TIME_WAIT` socket on daemon restart)
	/// from waking up in lockstep and immediately re-colliding.
	pub jitter: f64,
}

impl Default for Policy {
	fn default() -> Self {
		Self {
			max_attempts: 10,
			initial: Duration::from_millis(100),
			max: Duration::from_secs(5),
			jitter: 0.2,
		}
	}
}

/// Compute the next delay: `base` doubled, capped at `max`, then
/// scaled by a uniform jitter factor in `[1 - jitter/2, 1 + jitter/2]`
/// and re-clamped to `[initial, max]`. Pure function so tests can
/// pin behaviour deterministically by setting `jitter = 0.0`.
fn next_delay(base: Duration, policy: &Policy) -> Duration {
	let doubled = base.saturating_mul(2).min(policy.max);
	if policy.jitter <= f64::EPSILON {
		return doubled;
	}
	let half = policy.jitter / 2.0;
	let factor = 1.0 + (fastrand::f64() * policy.jitter - half);
	#[allow(
		clippy::cast_precision_loss,
		clippy::cast_possible_truncation,
		clippy::cast_sign_loss,
		reason = "jitter math: a backoff delay is bounded by `policy.max` (hours at most); f64 loses precision past 2^53 ns ≈ 104 days, well beyond any realistic max"
	)]
	let nanos = (doubled.as_nanos() as f64) * factor;
	#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
	let scaled = Duration::from_nanos(nanos.max(0.0) as u64);
	scaled.clamp(policy.initial, policy.max)
}

/// Bind a `TcpListener` with exponential backoff and cancellation.
///
/// On each attempt: create a `TcpSocket` matching the address family,
/// set `SO_REUSEADDR` (best-effort — silently ignored on platforms
/// where it is not permitted), `bind`, then `listen(backlog)`. If any
/// step fails, log a `warn`, sleep for the current backoff (capped at
/// `policy.max`), double the delay, and retry until `policy.max_attempts`
/// is exhausted. Returns `None` if cancellation fires mid-loop or
/// retries run out.
pub async fn tcp(
	addr: SocketAddr,
	cancel: &CancellationToken,
	policy: &Policy,
	backlog: u32,
) -> Option<TcpListener> {
	let mut delay = policy.initial;
	for attempt in 0..policy.max_attempts {
		if cancel.is_cancelled() {
			return None;
		}
		let socket_res = match addr {
			SocketAddr::V4(_) => TcpSocket::new_v4(),
			SocketAddr::V6(_) => TcpSocket::new_v6(),
		};
		let socket = match socket_res {
			Ok(s) => s,
			Err(e) => {
				tracing::warn!(?addr, attempt, ?e, "tcp socket creation failed");
				if sleep_or_cancel(delay, cancel).await {
					return None;
				}
				delay = next_delay(delay, policy);
				continue;
			}
		};
		let _ = socket.set_reuseaddr(true);
		match socket.bind(addr) {
			Ok(()) => match socket.listen(backlog) {
				Ok(l) => return Some(l),
				Err(e) => {
					tracing::warn!(?addr, attempt, ?e, "tcp listen failed");
				}
			},
			Err(e) => {
				tracing::warn!(?addr, attempt, ?e, "tcp bind failed");
			}
		}
		if sleep_or_cancel(delay, cancel).await {
			return None;
		}
		delay = next_delay(delay, policy);
	}
	None
}

/// Bind a `UdpSocket` with exponential backoff and cancellation.
///
/// Returns `None` if cancellation fires mid-loop or retries run out.
pub async fn udp(
	addr: SocketAddr,
	cancel: &CancellationToken,
	policy: &Policy,
) -> Option<UdpSocket> {
	let mut attempt: u32 = 0;
	let mut delay = policy.initial;
	loop {
		tokio::select! {
			biased;
			() = cancel.cancelled() => return None,
			res = UdpSocket::bind(addr) => match res {
				Ok(s) => return Some(s),
				Err(e) => {
					attempt = attempt.saturating_add(1);
					tracing::warn!(?addr, attempt, error = %e, "udp bind retry");
					if attempt >= policy.max_attempts {
						return None;
					}
					if sleep_or_cancel(delay, cancel).await {
						return None;
					}
					delay = next_delay(delay, policy);
				}
			}
		}
	}
}

/// Cancel-aware sleep. Returns `true` if cancellation cut the sleep
/// short, `false` if the sleep elapsed normally.
///
/// Useful in non-bind retry loops (e.g. an `accept` loop backing off
/// on `EMFILE`) that share the same cancellation token.
pub async fn sleep_or_cancel(delay: Duration, cancel: &CancellationToken) -> bool {
	tokio::select! {
		biased;
		() = cancel.cancelled() => true,
		() = tokio::time::sleep(delay) => false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::net::Ipv4Addr;
	use std::time::Instant;

	fn unused_addr() -> SocketAddr {
		SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
	}

	#[tokio::test]
	async fn tcp_bind_succeeds_first_try() {
		let cancel = CancellationToken::new();
		let policy = Policy::default();
		let listener = tcp(unused_addr(), &cancel, &policy, 64).await;
		assert!(listener.is_some());
	}

	#[tokio::test]
	async fn udp_bind_succeeds_first_try() {
		let cancel = CancellationToken::new();
		let policy = Policy::default();
		let socket = udp(unused_addr(), &cancel, &policy).await;
		assert!(socket.is_some());
	}

	#[tokio::test]
	async fn tcp_bind_gives_up_after_max_attempts() {
		// Bind once, then try to bind to the same port — second attempt
		// should hit EADDRINUSE every time and exhaust retries.
		let cancel = CancellationToken::new();
		let first = tcp(unused_addr(), &cancel, &Policy::default(), 64).await.unwrap();
		let busy = first.local_addr().unwrap();
		let policy = Policy {
			max_attempts: 2,
			initial: Duration::from_millis(1),
			max: Duration::from_millis(2),
			jitter: 0.0,
		};
		let result = tcp(busy, &cancel, &policy, 64).await;
		assert!(result.is_none());
	}

	#[tokio::test]
	async fn cancellation_aborts_during_backoff() {
		let cancel = CancellationToken::new();
		let first = tcp(unused_addr(), &cancel, &Policy::default(), 64).await.unwrap();
		let busy = first.local_addr().unwrap();

		let policy = Policy {
			max_attempts: 100,
			initial: Duration::from_mins(1),
			max: Duration::from_mins(1),
			jitter: 0.0,
		};
		let cancel_clone = cancel.clone();
		tokio::spawn(async move {
			tokio::time::sleep(Duration::from_millis(20)).await;
			cancel_clone.cancel();
		});
		let started = Instant::now();
		let result = tcp(busy, &cancel, &policy, 64).await;
		assert!(result.is_none());
		assert!(started.elapsed() < Duration::from_secs(5));
	}

	#[tokio::test]
	async fn sleep_or_cancel_returns_true_on_cancel() {
		let cancel = CancellationToken::new();
		cancel.cancel();
		let cut_short = sleep_or_cancel(Duration::from_mins(1), &cancel).await;
		assert!(cut_short);
	}

	#[tokio::test]
	async fn sleep_or_cancel_returns_false_on_elapse() {
		let cancel = CancellationToken::new();
		let cut_short = sleep_or_cancel(Duration::from_millis(1), &cancel).await;
		assert!(!cut_short);
	}

	#[test]
	fn next_delay_zero_jitter_doubles_capped_at_max() {
		let policy = Policy {
			max_attempts: 10,
			initial: Duration::from_millis(100),
			max: Duration::from_millis(500),
			jitter: 0.0,
		};
		assert_eq!(next_delay(Duration::from_millis(100), &policy), Duration::from_millis(200));
		assert_eq!(next_delay(Duration::from_millis(200), &policy), Duration::from_millis(400));
		// Doubled would be 800 ms; max-cap clamps to 500 ms.
		assert_eq!(next_delay(Duration::from_millis(400), &policy), Duration::from_millis(500));
	}

	#[test]
	fn next_delay_with_jitter_stays_within_band() {
		let policy = Policy {
			max_attempts: 10,
			initial: Duration::from_millis(10),
			max: Duration::from_secs(1),
			jitter: 0.2,
		};
		// Doubled is 200 ms; ±10% band is [180, 220] ms after jitter.
		// Run a handful of samples to exercise the random factor.
		for _ in 0..32 {
			let d = next_delay(Duration::from_millis(100), &policy);
			assert!(
				d >= Duration::from_millis(180) && d <= Duration::from_millis(220),
				"jittered delay {d:?} outside band"
			);
		}
	}
}
