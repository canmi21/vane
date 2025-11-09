/* src/modules/stack/transport/session.rs */

use super::model::Target;
use crate::common::getenv;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use std::mem;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{Duration, Instant};

/// Represents a client's session, mapping to a specific upstream socket.
pub struct Session {
	pub target: Target,
	pub upstream_socket: Arc<UdpSocket>,
	pub last_seen: Instant,
}

/// Maps a client's address to their active session.
pub static SESSIONS: Lazy<DashMap<SocketAddr, Arc<Session>>> = Lazy::new(DashMap::new);
/// Maps an upstream socket's local address back to the client's address for replies.
pub static REVERSE_SESSIONS: Lazy<DashMap<SocketAddr, SocketAddr>> = Lazy::new(DashMap::new);

/// Spawns a background task to clean up expired UDP sessions.
pub fn start_session_cleanup_task() {
	log(LogLevel::Debug, "⚙ Starting UDP session cleanup task...");

	let buffer_limit_str = getenv::get_env("UDP_SESSION_BUFFER", "4194304".to_string());
	let buffer_limit = buffer_limit_str.parse::<usize>().unwrap_or(4_194_304);

	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(10));
		loop {
			interval.tick().await;
			let session_timeout = Duration::from_secs(30);
			let now = Instant::now();
			let mut expired_keys = Vec::new();

			// First pass: collect expired keys.
			for entry in SESSIONS.iter() {
				if now.duration_since(entry.value().last_seen) > session_timeout {
					expired_keys.push(*entry.key());
				}
			}

			// Second pass: remove expired sessions.
			for key in expired_keys {
				if let Some((_, session)) = SESSIONS.remove(&key) {
					if let Ok(addr) = session.upstream_socket.local_addr() {
						REVERSE_SESSIONS.remove(&addr);
					}
				}
			}

			// Third pass: enforce memory buffer limit if still over.
			let current_size =
				SESSIONS.len() * (mem::size_of::<SocketAddr>() + mem::size_of::<Arc<Session>>());
			if current_size > buffer_limit {
				log(
					LogLevel::Warn,
					&format!(
						"UDP session buffer limit exceeded ({} > {}). Pruning oldest sessions.",
						current_size, buffer_limit
					),
				);
				let mut all_sessions: Vec<_> = SESSIONS
					.iter()
					.map(|e| (*e.key(), e.value().last_seen))
					.collect();
				all_sessions.sort_by_key(|a| a.1);
				let to_prune_count = (SESSIONS.len() as f64 * 0.1).ceil() as usize; // Prune 10%
				for (key, _) in all_sessions.iter().take(to_prune_count) {
					if let Some((_, session)) = SESSIONS.remove(key) {
						if let Ok(addr) = session.upstream_socket.local_addr() {
							REVERSE_SESSIONS.remove(&addr);
						}
					}
				}
			}
		}
	});
}
