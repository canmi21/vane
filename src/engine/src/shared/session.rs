use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use std::mem;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{Duration, Instant};
use vane_primitives::model::ResolvedTarget;
use vane_primitives::tasks::ConnectionGuard;

pub struct Session {
	pub target: ResolvedTarget,
	pub upstream_socket: Arc<UdpSocket>,
	pub last_seen: Instant,
	pub _guard: ConnectionGuard,
}

/// A globally shared, thread-safe map for UDP sessions.
/// The key is a tuple of (client_address, protocol_name) to ensure
/// that traffic from a single client can be routed to different backends
/// based on the matched protocol rule.
pub static SESSIONS: Lazy<DashMap<(SocketAddr, String), Arc<Session>>> = Lazy::new(DashMap::new);

/// A reverse mapping from an upstream socket's ephemeral address back to the client's address.
/// This is essential for routing replies correctly.
pub static REVERSE_SESSIONS: Lazy<DashMap<SocketAddr, SocketAddr>> = Lazy::new(DashMap::new);

/// Spawns a background task to clean up expired UDP sessions.
/// The session timeout is configurable via the `UDP_SESSION_TIMEOUT_SECS` environment variable.
pub fn start_session_cleanup_task() {
	log(LogLevel::Debug, "⚙ Starting UDP session cleanup task...");
	let buffer_limit = envflag::get::<usize>("UDP_SESSION_BUFFER", 4_194_304);

	tokio::spawn(async move {
		let session_timeout_secs = envflag::get::<u64>("UDP_SESSION_TIMEOUT_SECS", 30);
		let session_timeout = Duration::from_secs(session_timeout_secs);

		let mut interval = tokio::time::interval(Duration::from_secs(10));
		loop {
			interval.tick().await;
			let now = Instant::now();
			let mut expired_keys = Vec::new();

			for entry in SESSIONS.iter() {
				if now.duration_since(entry.value().last_seen) > session_timeout {
					expired_keys.push(entry.key().clone());
				}
			}

			for key in expired_keys {
				if let Some((_, session)) = SESSIONS.remove(&key)
					&& let Ok(addr) = session.upstream_socket.local_addr()
				{
					REVERSE_SESSIONS.remove(&addr);
				}
			}

			// Memory limit enforcement
			let current_size =
				SESSIONS.len() * (mem::size_of::<(SocketAddr, String)>() + mem::size_of::<Arc<Session>>());
			if current_size > buffer_limit {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ UDP session buffer limit exceeded ({current_size} > {buffer_limit}). Pruning oldest sessions."
					),
				);
				let mut all_sessions: Vec<_> =
					SESSIONS.iter().map(|e| (e.key().clone(), e.value().last_seen)).collect();
				all_sessions.sort_by_key(|a| a.1);
				let to_prune_count = (SESSIONS.len() as f64 * 0.1).ceil() as usize;
				for (key, _) in all_sessions.iter().take(to_prune_count) {
					if let Some((_, session)) = SESSIONS.remove(key)
						&& let Ok(addr) = session.upstream_socket.local_addr()
					{
						REVERSE_SESSIONS.remove(&addr);
					}
				}
			}
		}
	});
}
