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
}

impl Default for Policy {
	fn default() -> Self {
		Self { max_attempts: 10, initial: Duration::from_millis(100), max: Duration::from_secs(5) }
	}
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
				delay = (delay * 2).min(policy.max);
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
		delay = (delay * 2).min(policy.max);
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
					delay = (delay * 2).min(policy.max);
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
		let policy =
			Policy { max_attempts: 2, initial: Duration::from_millis(1), max: Duration::from_millis(2) };
		let result = tcp(busy, &cancel, &policy, 64).await;
		assert!(result.is_none());
	}

	#[tokio::test]
	async fn cancellation_aborts_during_backoff() {
		let cancel = CancellationToken::new();
		let first = tcp(unused_addr(), &cancel, &Policy::default(), 64).await.unwrap();
		let busy = first.local_addr().unwrap();

		let policy =
			Policy { max_attempts: 100, initial: Duration::from_mins(1), max: Duration::from_mins(1) };
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
}
