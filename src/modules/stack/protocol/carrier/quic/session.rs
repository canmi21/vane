/* src/modules/stack/protocol/carrier/quic/session.rs */

use crate::common::getenv;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;

#[derive(Debug, Clone)]
pub enum SessionAction {
	// Store the NAT socket used for this session "upstream_socket"
	Forward {
		target_addr: SocketAddr,
		upstream_socket: Arc<UdpSocket>,
		last_seen: Instant,
	},
	Terminate {
		muxer_port: u16,
		last_seen: Instant,
	},
}

#[derive(Debug, Clone)]
pub struct PendingState {
	// Reassembled stream data (Offset -> Data)
	pub crypto_stream: BTreeMap<usize, Vec<u8>>,
	// Buffered packets (Data, ClientAddr, DstAddr)
	pub queued_packets: Vec<(Vec<u8>, SocketAddr, SocketAddr)>,
	pub last_seen: Instant,
}

/// Global registry mapping Connection IDs (DCID) to Actions.
pub static CID_REGISTRY: Lazy<DashMap<Vec<u8>, SessionAction>> = Lazy::new(|| DashMap::new());

/// Registry for pending Initials waiting for SNI.
pub static PENDING_INITIALS: Lazy<DashMap<Vec<u8>, PendingState>> = Lazy::new(|| DashMap::new());

/// IP Stickiness Map: ClientAddr -> (TargetAddr, UpstreamSocket, LastSeen)
/// Used when CID lookup fails (e.g. server-initiated CID migration in Transparent Proxy).
pub static IP_STICKY_MAP: Lazy<DashMap<SocketAddr, (SocketAddr, Arc<UdpSocket>, Instant)>> =
	Lazy::new(|| DashMap::new());

pub fn register_session(cid: Vec<u8>, action: SessionAction) {
	PENDING_INITIALS.remove(&cid);
	CID_REGISTRY.insert(cid, action);
}

pub fn register_sticky(client: SocketAddr, target: SocketAddr, socket: Arc<UdpSocket>) {
	IP_STICKY_MAP.insert(client, (target, socket, Instant::now()));
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
	PENDING_INITIALS.retain(|_, state| now.duration_since(state.last_seen).as_secs() < 10);

	// Cleanup Sticky Sessions
	// Default: 60 seconds (generous for NAT rebinding/migration)
	let sticky_timeout_str = getenv::get_env("QUIC_STICKY_SESSION_TTL", "60".to_string());
	let sticky_timeout = sticky_timeout_str.parse::<u64>().unwrap_or(60);

	IP_STICKY_MAP.retain(|_, (_, _, last)| now.duration_since(*last).as_secs() < sticky_timeout);
}
