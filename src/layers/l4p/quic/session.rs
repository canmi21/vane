/* src/layers/l4p/quic/session.rs */

use crate::common::config::getenv;
use crate::ingress::tasks::ConnectionGuard;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tokio::net::UdpSocket;

// --- Resource Management ---

// Global counter for total buffered QUIC bytes across all pending sessions
static GLOBAL_PENDING_BYTES: AtomicUsize = AtomicUsize::new(0);

// Default: 64MB global limit
fn get_global_byte_limit() -> usize {
	getenv::get_env("QUIC_GLOBAL_PENDING_BYTES_LIMIT", "67108864".to_string())
		.parse()
		.unwrap_or(67_108_864)
}

// Default: 64KB per session limit (enough for massive fragmented ClientHello)
fn get_session_byte_limit() -> usize {
	getenv::get_env("QUIC_SESSION_BUFFER_LIMIT", "65536".to_string())
		.parse()
		.unwrap_or(65_536)
}

/// Tries to reserve global bytes. Returns true if successful.
pub fn try_reserve_global_bytes(amount: usize) -> bool {
	let limit = get_global_byte_limit();
	let current = GLOBAL_PENDING_BYTES.load(Ordering::Relaxed);
	if current + amount > limit {
		log(
			LogLevel::Warn,
			&format!(
				"⚠ QUIC Global Buffer Limit Exceeded! Dropping {} bytes (Current: {}/{})",
				amount, current, limit
			),
		);
		return false;
	}
	GLOBAL_PENDING_BYTES.fetch_add(amount, Ordering::Relaxed);
	true
}

/// Releases global bytes.
pub fn release_global_bytes(amount: usize) {
	GLOBAL_PENDING_BYTES.fetch_sub(amount, Ordering::Relaxed);
}

#[derive(Debug, Clone)]
pub enum SessionAction {
	// Store the NAT socket used for this session "upstream_socket"
	Forward {
		target_addr: SocketAddr,
		upstream_socket: Arc<UdpSocket>,
		last_seen: Instant,
		_guard: ConnectionGuard,
	},
	Terminate {
		muxer_port: u16,
		last_seen: Instant,
		_guard: Option<ConnectionGuard>,
	},
}

#[derive(Debug)]
pub struct PendingState {
	// Reassembled stream data (Offset -> Data)
	pub crypto_stream: BTreeMap<usize, Vec<u8>>,
	// Buffered packets (Data, ClientAddr, DstAddr)
	pub queued_packets: Vec<(bytes::Bytes, SocketAddr, SocketAddr)>,
	pub last_seen: Instant,
	/// Flag to ensure only one task proceeds to flow execution
	pub processing: bool,
	pub _guard: ConnectionGuard,
	/// Total bytes currently buffered in this session
	pub total_bytes: usize,
}

impl PendingState {
	/// Safely drains the queued packets, reducing total_bytes and releasing global quota accordingly.
	/// This is required because PendingState implements Drop.
	pub fn drain_queue(&mut self) -> Vec<(bytes::Bytes, SocketAddr, SocketAddr)> {
		let packets = std::mem::take(&mut self.queued_packets);
		let drained_size: usize = packets.iter().map(|(data, _, _)| data.len()).sum();

		self.total_bytes = self.total_bytes.saturating_sub(drained_size);
		release_global_bytes(drained_size);
		packets
	}
}

impl Drop for PendingState {
	fn drop(&mut self) {
		if self.total_bytes > 0 {
			release_global_bytes(self.total_bytes);
		}
	}
}

/// Global registry mapping Connection IDs (DCID) to Actions.
pub static CID_REGISTRY: Lazy<DashMap<Vec<u8>, SessionAction>> = Lazy::new(|| DashMap::new());

/// Registry for pending Initials waiting for SNI.
pub static PENDING_INITIALS: Lazy<DashMap<Vec<u8>, PendingState>> = Lazy::new(|| DashMap::new());

/// IP Stickiness Map: ClientAddr -> (TargetAddr, UpstreamSocket, LastSeen, Guard)
/// Used when CID lookup fails (e.g. server-initiated CID migration in Transparent Proxy).
pub static IP_STICKY_MAP: Lazy<
	DashMap<SocketAddr, (SocketAddr, Arc<UdpSocket>, Instant, ConnectionGuard)>,
> = Lazy::new(|| DashMap::new());

pub fn register_session(cid: Vec<u8>, action: SessionAction) {
	// Removal from PENDING_INITIALS triggers Drop, releasing bytes automatically.
	PENDING_INITIALS.remove(&cid);
	CID_REGISTRY.insert(cid, action);
}

pub fn register_sticky(
	client: SocketAddr,
	target: SocketAddr,
	socket: Arc<UdpSocket>,
	guard: ConnectionGuard,
) {
	IP_STICKY_MAP.insert(client, (target, socket, Instant::now(), guard));
}

pub fn get_sticky(client: &SocketAddr) -> Option<(SocketAddr, Arc<UdpSocket>)> {
	if let Some(mut entry) = IP_STICKY_MAP.get_mut(client) {
		entry.2 = Instant::now(); // Update last_seen
		return Some((entry.0, entry.1.clone()));
	}
	None
}

pub fn get_session(cid: &[u8]) -> Option<SessionAction> {
	CID_REGISTRY.get(cid).map(|r| r.value().clone())
}

pub fn touch_session(cid: &[u8]) {
	if let Some(mut entry) = CID_REGISTRY.get_mut(cid) {
		match entry.value_mut() {
			SessionAction::Forward { last_seen, .. } => *last_seen = Instant::now(),
			SessionAction::Terminate { last_seen, .. } => *last_seen = Instant::now(),
		}
	}
}

pub fn check_session_limit(current: usize, add: usize) -> bool {
	let limit = get_session_byte_limit();
	if current + add > limit {
		log(
			LogLevel::Warn,
			&format!(
				"⚠ QUIC Session Buffer Limit Exceeded! Dropping (Current: {}/{})",
				current, limit
			),
		);
		return false;
	}
	true
}

pub fn cleanup_sessions(timeout_secs: u64) {
	let now = Instant::now();

	// Cleanup CID Sessions
	CID_REGISTRY.retain(|_, action| {
		let last = match action {
			SessionAction::Forward { last_seen, .. } => last_seen,
			SessionAction::Terminate { last_seen, .. } => last_seen,
		};
		now.duration_since(*last).as_secs() < timeout_secs
	});

	// Cleanup Pending Initials (Strict 10s)
	// Removal triggers Drop -> release_global_bytes
	PENDING_INITIALS.retain(|_, state| now.duration_since(state.last_seen).as_secs() < 10);

	// Cleanup Sticky Sessions
	// Default: 60 seconds (generous for NAT rebinding/migration)
	let sticky_timeout_str = getenv::get_env("QUIC_STICKY_SESSION_TTL", "60".to_string());
	let sticky_timeout = sticky_timeout_str.parse::<u64>().unwrap_or(60);

	IP_STICKY_MAP.retain(|_, (_, _, last, _)| now.duration_since(*last).as_secs() < sticky_timeout);
}

/// Spawns a background task to clean up expired QUIC sessions.
pub fn start_cleanup_task() {
	use tokio::time::{Duration, sleep};

	log(LogLevel::Debug, "⚙ Starting QUIC session cleanup task...");

	tokio::spawn(async move {
		let ttl_str = getenv::get_env("QUIC_SESSION_TTL_SECS", "300".to_string());
		let ttl = ttl_str.parse::<u64>().unwrap_or(300);
		let check_interval = Duration::from_secs(ttl / 2);

		loop {
			sleep(check_interval).await;
			cleanup_sessions(ttl);
		}
	});
}
