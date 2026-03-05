/* src/extra/src/l4/proxy/forwarder.rs */

use anyhow::{Context, Result};
use fancy_log::{LogLevel, log};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::{
	io,
	net::{TcpStream, UdpSocket},
	// Here we use Std::Instant, so do not import tokio one to avoid ambiguity
	time::{Duration, timeout},
};
use vane_engine::shared::health;
use vane_engine::shared::session::{REVERSE_SESSIONS, SESSIONS, Session};
use vane_primitives::common::net::ip;
use vane_primitives::model::ResolvedTarget;
use vane_transport::l4p::quic::session::{self, SessionAction};
use vane_transport::protocol::quic::parser;

// Constants
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

// TCP Logic
pub async fn proxy_tcp_stream(mut client_stream: TcpStream, target: ResolvedTarget) -> Result<()> {
	let peer_addr =
		client_stream.peer_addr().map_or_else(|_| "unknown".to_owned(), |a| a.to_string());
	let target_str = format!("{}:{}", target.ip, target.port);

	log(LogLevel::Debug, &format!("➜ Proxying TCP connection from {peer_addr} to {target_str}"));

	match timeout(CONNECT_TIMEOUT, TcpStream::connect((target.ip.as_str(), target.port))).await {
		Ok(Ok(mut upstream_stream)) => {
			let _ = client_stream.set_nodelay(true);
			let _ = upstream_stream.set_nodelay(true);

			let (mut client_read, mut client_write) = client_stream.split();
			let (mut upstream_read, mut upstream_write) = upstream_stream.split();

			let client_to_server = tokio::io::copy(&mut client_read, &mut upstream_write);
			let server_to_client = tokio::io::copy(&mut upstream_read, &mut client_write);

			tokio::select! {
				res = client_to_server => res.map(|_| ()).context("Client->Server copy failed"),
				res = server_to_client => res.map(|_| ()).context("Server->Client copy failed"),
			}
		}
		Ok(Err(e)) => {
			log(LogLevel::Error, &format!("✗ Failed to connect to upstream target {target_str}: {e}"));
			health::mark_tcp_target_unhealthy(&target);
			Err(anyhow::Error::new(e))
		}
		Err(_) => {
			log(LogLevel::Error, &format!("✗ Timeout connecting to upstream target {target_str}"));
			health::mark_tcp_target_unhealthy(&target);
			Err(anyhow::anyhow!("Connection timed out"))
		}
	}
}

pub async fn proxy_generic_stream(
	client_stream: Box<dyn vane_engine::engine::interfaces::ByteStream>,
	target: ResolvedTarget,
) -> Result<()> {
	log(
		LogLevel::Debug,
		&format!("➜ Generic Stream Proxy to upstream: {}:{}", target.ip, target.port),
	);

	match timeout(CONNECT_TIMEOUT, TcpStream::connect(format!("{}:{}", target.ip, target.port))).await
	{
		Ok(Ok(mut upstream_stream)) => {
			let _ = upstream_stream.set_nodelay(true);

			let (mut client_read, mut client_write) = tokio::io::split(client_stream);
			let (mut upstream_read, mut upstream_write) = upstream_stream.split();

			let client_to_server = tokio::io::copy(&mut client_read, &mut upstream_write);
			let server_to_client = tokio::io::copy(&mut upstream_read, &mut client_write);

			tokio::select! {
				res = client_to_server => res.map(|_| ()).context("L4+ Client->Server copy failed"),
				res = server_to_client => res.map(|_| ()).context("L4+ Server->Client copy failed"),
			}
		}
		Ok(Err(e)) => {
			health::mark_tcp_target_unhealthy(&target);
			Err(anyhow::Error::new(e).context("Failed to connect to upstream"))
		}
		Err(_) => {
			health::mark_tcp_target_unhealthy(&target);
			Err(anyhow::anyhow!("Connection timed out"))
		}
	}
}

// UDP Logic
async fn bind_upstream_socket(target_ip: &IpAddr) -> Result<UdpSocket, std::io::Error> {
	let bind_addr: SocketAddr =
		if target_ip.is_ipv6() { ([0; 16], 0).into() } else { ([0; 4], 0).into() };
	UdpSocket::bind(bind_addr).await
}

fn spawn_reply_handler(
	upstream_socket: Arc<UdpSocket>,
	main_socket: Arc<UdpSocket>,
	timeout: Duration,
) {
	tokio::spawn(async move {
		let mut buf = [0; 65535];
		if let Ok(local_addr) = upstream_socket.local_addr() {
			loop {
				if let Ok(Ok((len, _))) =
					tokio::time::timeout(timeout, upstream_socket.recv_from(&mut buf)).await
				{
					if let Some(client_addr) = REVERSE_SESSIONS.get(&local_addr)
						&& main_socket.send_to(&buf[..len], *client_addr).await.is_err()
					{
						break;
					}
				} else {
					if let Some((_, _client_addr)) = REVERSE_SESSIONS.remove(&local_addr) {}
					break;
				}
			}
		}
	});
}

pub async fn proxy_udp_direct(
	main_socket: Arc<UdpSocket>,
	datagram: &bytes::Bytes,
	client_addr: SocketAddr,
	target: ResolvedTarget,
) -> Result<()> {
	let nat_key = format!("flow:{}:{}", target.ip, target.port);
	let session_key = (client_addr, nat_key.clone());

	if let Some((_, session)) = SESSIONS.remove(&session_key) {
		if health::is_udp_target_healthy(&session.target) {
			let updated_session = Arc::new(Session {
				target: session.target.clone(),
				upstream_socket: session.upstream_socket.clone(),
				// FIX: Use tokio::time::Instant explicitly
				last_seen: tokio::time::Instant::now(),
				_guard: session._guard.clone(),
			});
			SESSIONS.insert(session_key.clone(), updated_session.clone());

			let target_addr = format!("{}:{}", updated_session.target.ip, updated_session.target.port);
			let send_result = updated_session.upstream_socket.send_to(datagram, &target_addr).await;

			if send_result.is_err() {
				health::mark_udp_target_unhealthy(&updated_session.target);
				if let Ok(addr) = updated_session.upstream_socket.local_addr() {
					REVERSE_SESSIONS.remove(&addr);
				}
				SESSIONS.remove(&session_key);
				return Err(anyhow::Error::new(io::Error::new(
					io::ErrorKind::ConnectionReset,
					"Failed to send to upstream",
				)));
			}
			return Ok(());
		} else if let Ok(addr) = session.upstream_socket.local_addr() {
			REVERSE_SESSIONS.remove(&addr);
		}
	}

	let target_ip = target.ip.parse::<IpAddr>().context("Invalid target IP")?;
	let upstream_socket =
		bind_upstream_socket(&target_ip).await.context("Failed to bind upstream socket")?;
	let upstream_arc = Arc::new(upstream_socket);

	if let Ok(local_addr) = upstream_arc.local_addr() {
		// Apply Connection Rate Limits
		let Some(guard) = vane_primitives::tasks::GLOBAL_TRACKER.acquire(client_addr.ip()) else {
			log(
				LogLevel::Debug,
				&format!(
					"⚙ Rate limited UDP Flow session from {} to {}:{}",
					client_addr, target.ip, target.port
				),
			);
			return Err(anyhow::anyhow!("Rate limited"));
		};

		let new_session = Arc::new(Session {
			target: target.clone(),
			upstream_socket: upstream_arc.clone(),
			// FIX: Use tokio::time::Instant explicitly
			last_seen: tokio::time::Instant::now(),
			_guard: guard,
		});

		SESSIONS.insert(session_key, new_session.clone());
		REVERSE_SESSIONS.insert(local_addr, client_addr);

		let timeout_ms = if ip::is_private_ip(&target_ip) {
			envflag::get::<u64>("UDP_TIMEOUT_LOCAL", 500)
		} else {
			envflag::get::<u64>("UDP_TIMEOUT_REMOTE", 5000)
		};

		spawn_reply_handler(upstream_arc.clone(), main_socket, Duration::from_millis(timeout_ms));

		let target_addr = format!("{}:{}", target.ip, target.port);
		let send_result = upstream_arc.send_to(datagram, &target_addr).await;
		send_result.context("Failed to forward initial UDP packet")?;

		log(LogLevel::Debug, &format!("➜ Established UDP NAT mapping: {client_addr} <-> {nat_key}"));
		return Ok(());
	}

	Err(anyhow::anyhow!("Failed to create UDP NAT mapping"))
}

// --- QUIC Specific Logic ---

fn spawn_quic_reply_handler(
	upstream_socket: Arc<UdpSocket>,
	listener_socket: Arc<UdpSocket>,
	timeout_duration: Duration,
) {
	let buf_size = envflag::get::<usize>("QUIC_RECV_BUFFER_SIZE", 65535);

	tokio::spawn(async move {
		let mut buf = vec![0u8; buf_size];

		if let Ok(local_addr) = upstream_socket.local_addr() {
			loop {
				if let Ok(Ok((len, _))) =
					timeout(timeout_duration, upstream_socket.recv_from(&mut buf)).await
				{
					if let Some(client_addr) = REVERSE_SESSIONS.get(&local_addr) {
						let _ = listener_socket.send_to(&buf[..len], *client_addr).await;
					}
				} else {
					if let Some((_, _client_addr)) = REVERSE_SESSIONS.remove(&local_addr) {}
					break;
				}
			}
		}
	});
}

/// Handles QUIC packet forwarding using the global L4 Session table.
#[allow(clippy::too_many_lines)]
pub async fn proxy_quic_association(
	listener_socket: Arc<UdpSocket>,
	datagram: &bytes::Bytes,
	client_addr: SocketAddr,
	target: ResolvedTarget,
) -> Result<()> {
	let nat_key = format!("quic:{}:{}", target.ip, target.port);
	let session_key = (client_addr, nat_key.clone());

	// 1. Existing NAT Session Logic (Refresh)
	if let Some((_, session)) = SESSIONS.remove(&session_key) {
		if health::is_udp_target_healthy(&session.target) {
			// Update UDP Session
			let updated_session = Arc::new(Session {
				target: session.target.clone(),
				upstream_socket: session.upstream_socket.clone(),
				// FIX: Use tokio::time::Instant
				last_seen: tokio::time::Instant::now(),
				_guard: session._guard.clone(),
			});
			SESSIONS.insert(session_key.clone(), updated_session.clone());

			let target_addr = format!("{}:{}", updated_session.target.ip, updated_session.target.port);

			// Forward current packet
			let send_result = updated_session.upstream_socket.send_to(datagram, &target_addr).await;

			// --- FIX: Refresh Sticky Session (Keepalive) ---
			// Ensure L4 fallback keeps working using the correct upstream socket
			if let Ok(target_socket_addr) = target_addr.parse::<SocketAddr>() {
				session::register_sticky(
					client_addr,
					target_socket_addr,
					updated_session.upstream_socket.clone(),
					updated_session._guard.clone(),
				);
			}

			if let Err(e) = send_result {
				health::mark_udp_target_unhealthy(&updated_session.target);
				if let Ok(addr) = updated_session.upstream_socket.local_addr() {
					REVERSE_SESSIONS.remove(&addr);
				}
				SESSIONS.remove(&session_key);
				return Err(
					anyhow::Error::new(e).context("Failed to forward QUIC packet on existing session"),
				);
			}
			return Ok(());
		} else if let Ok(addr) = session.upstream_socket.local_addr() {
			REVERSE_SESSIONS.remove(&addr);
		}
	}

	// 2. New Session Logic
	let bind_addr: SocketAddr =
		if target.ip.contains(':') { ([0; 16], 0).into() } else { ([0; 4], 0).into() };

	let upstream_socket =
		UdpSocket::bind(bind_addr).await.context("Failed to bind ephemeral socket for QUIC")?;
	let upstream_arc = Arc::new(upstream_socket);

	if let Ok(local_addr) = upstream_arc.local_addr() {
		// Apply Connection Rate Limits
		let Some(guard) = vane_primitives::tasks::GLOBAL_TRACKER.acquire(client_addr.ip()) else {
			log(
				LogLevel::Debug,
				&format!(
					"⚙ Rate limited QUIC Flow session from {} to {}:{}",
					client_addr, target.ip, target.port
				),
			);
			return Err(anyhow::anyhow!("Rate limited"));
		};

		// Register UDP Session
		let new_session = Arc::new(Session {
			target: target.clone(),
			upstream_socket: upstream_arc.clone(),
			// FIX: Use tokio::time::Instant
			last_seen: tokio::time::Instant::now(),
			_guard: guard,
		});

		SESSIONS.insert(session_key, new_session.clone());
		REVERSE_SESSIONS.insert(local_addr, client_addr);

		let target_ip_parsed = target.ip.parse::<IpAddr>().unwrap_or_else(|_| {
			if target.ip.contains(':') {
				IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1])
			} else {
				IpAddr::from([127, 0, 0, 1])
			}
		});

		let timeout_ms = if ip::is_private_ip(&target_ip_parsed) {
			envflag::get::<u64>("QUIC_TIMEOUT_LOCAL", 1000)
		} else {
			envflag::get::<u64>("QUIC_TIMEOUT_REMOTE", 10000)
		};

		// Start background reply handler
		spawn_quic_reply_handler(
			upstream_arc.clone(),
			listener_socket,
			Duration::from_millis(timeout_ms),
		);

		let target_addr_str = format!("{}:{}", target.ip, target.port);
		let target_socket_addr =
			target_addr_str.parse::<SocketAddr>().context("Invalid Target Addr")?;

		// --- INTEGRATION: Register L4+ Session (Fast Path + Sticky) ---
		if let Some(dcid) = parser::peek_long_header_dcid(datagram) {
			// 1. Register CID (Std Instant)
			session::register_session(
				dcid.clone(),
				SessionAction::Forward {
					target_addr: target_socket_addr,
					// FIX: Pass the upstream socket for consistency/validity
					upstream_socket: upstream_arc.clone(),
					last_seen: std::time::Instant::now(),
					_guard: new_session._guard.clone(),
				},
			);

			// 2. Register Sticky (Std Instant via internal session.rs call)
			session::register_sticky(
				client_addr,
				target_socket_addr,
				upstream_arc.clone(),
				new_session._guard.clone(),
			);

			log(
				LogLevel::Debug,
				&format!("⚙ Registered QUIC Forward Session for DCID len={}", dcid.len()),
			);

			// 3. Flush Queue
			if let Some((_, mut state)) = session::PENDING_INITIALS.remove(&dcid) {
				let packets = state.drain_queue();
				log(
					LogLevel::Debug,
					&format!("➜ Flushing {} buffered packets to Upstream Proxy", packets.len()),
				);

				for (pkt, _, _) in packets {
					if pkt == datagram {
						continue;
					}
					// FIX: Use upstream socket
					let _ = upstream_arc.send_to(&pkt, &target_addr_str).await;
				}
			}
		} else {
			// Fallback: If no Long Header, just register Sticky
			session::register_sticky(
				client_addr,
				target_socket_addr,
				upstream_arc.clone(),
				new_session._guard.clone(),
			);
		}

		// 5. Send the current packet
		let send_result = upstream_arc.send_to(datagram, &target_addr_str).await;

		send_result.context("Failed to forward initial QUIC packet")?;

		log(LogLevel::Debug, &format!("➜ Established QUIC NAT mapping: {client_addr} <-> {nat_key}"));
	}

	Ok(())
}
