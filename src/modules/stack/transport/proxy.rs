/* src/modules/stack/transport/proxy.rs */

use super::{
	health,
	model::ResolvedTarget,
	session::{REVERSE_SESSIONS, SESSIONS, Session},
};
use crate::common::{getenv, ip};
use anyhow::{Context, Result};
use fancy_log::{LogLevel, log};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::{
	io,
	net::{TcpStream, UdpSocket},
	time::{Duration, Instant, timeout},
};

// Constants
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

// --- TCP Logic ---

pub async fn proxy_tcp_stream(mut client_stream: TcpStream, target: ResolvedTarget) -> Result<()> {
	log(
		LogLevel::Debug,
		&format!(
			"➜ TCP Proxy connecting to upstream: {}:{}",
			target.ip, target.port
		),
	);

	// FIX: Restored explicit match/error handling to mark target unhealthy on failure.
	// Using '?' directly skipped the health marking logic.
	let connect_result = timeout(
		CONNECT_TIMEOUT,
		TcpStream::connect(format!("{}:{}", target.ip, target.port)),
	)
	.await;

	let mut upstream_stream = match connect_result {
		Ok(Ok(stream)) => stream,
		Ok(Err(e)) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to connect to upstream target {}:{}: {}",
					target.ip, target.port, e
				),
			);
			health::mark_tcp_target_unhealthy(&target);
			return Err(anyhow::Error::new(e).context("Failed to connect to upstream"));
		}
		Err(_) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Timeout connecting to upstream target {}:{}",
					target.ip, target.port
				),
			);
			health::mark_tcp_target_unhealthy(&target);
			return Err(anyhow::anyhow!("Connection timed out"));
		}
	};

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

pub async fn proxy_generic_stream(
	client_stream: Box<dyn crate::modules::plugins::model::ByteStream>,
	target: ResolvedTarget,
) -> Result<()> {
	log(
		LogLevel::Debug,
		&format!(
			"➜ Generic Stream Proxy to upstream: {}:{}",
			target.ip, target.port
		),
	);

	// FIX: Same fix applied here for Generic/L4+ streams
	let connect_result = timeout(
		CONNECT_TIMEOUT,
		TcpStream::connect(format!("{}:{}", target.ip, target.port)),
	)
	.await;

	let mut upstream_stream = match connect_result {
		Ok(Ok(stream)) => stream,
		Ok(Err(e)) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to connect to upstream {}:{}: {}",
					target.ip, target.port, e
				),
			);
			health::mark_tcp_target_unhealthy(&target);
			return Err(anyhow::Error::new(e).context("Failed to connect to upstream"));
		}
		Err(_) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Timeout connecting to upstream {}:{}",
					target.ip, target.port
				),
			);
			health::mark_tcp_target_unhealthy(&target);
			return Err(anyhow::anyhow!("Connection timed out"));
		}
	};

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

// --- UDP Logic ---

async fn bind_upstream_socket(target_ip: &IpAddr) -> Result<UdpSocket, std::io::Error> {
	let bind_addr: SocketAddr = if target_ip.is_ipv6() {
		([0; 16], 0).into()
	} else {
		([0; 4], 0).into()
	};
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
				match tokio::time::timeout(timeout, upstream_socket.recv_from(&mut buf)).await {
					Ok(Ok((len, _))) => {
						if let Some(client_addr) = REVERSE_SESSIONS.get(&local_addr) {
							if main_socket
								.send_to(&buf[..len], *client_addr)
								.await
								.is_err()
							{
								break;
							}
						}
					}
					_ => {
						if let Some((_, _client_addr)) = REVERSE_SESSIONS.remove(&local_addr) {}
						break;
					}
				}
			}
		}
	});
}

pub async fn proxy_udp_direct(
	main_socket: Arc<UdpSocket>,
	datagram: &[u8],
	client_addr: SocketAddr,
	target: ResolvedTarget,
) -> io::Result<()> {
	// Flow Engine / Plugin Logic
	let nat_key = format!("flow:{}:{}", target.ip, target.port);
	let session_key = (client_addr, nat_key.clone());

	if let Some((_, session)) = SESSIONS.remove(&session_key) {
		if health::is_udp_target_healthy(&session.target) {
			let updated_session = Arc::new(Session {
				target: session.target.clone(),
				upstream_socket: session.upstream_socket.clone(),
				last_seen: Instant::now(),
				_guard: session._guard.clone(),
			});
			SESSIONS.insert(session_key.clone(), updated_session.clone());

			let target_addr = (
				updated_session.target.ip.as_str(),
				updated_session.target.port,
			);
			if updated_session
				.upstream_socket
				.send_to(datagram, target_addr)
				.await
				.is_err()
			{
				health::mark_udp_target_unhealthy(&updated_session.target);
				if let Ok(addr) = updated_session.upstream_socket.local_addr() {
					REVERSE_SESSIONS.remove(&addr);
				}
				SESSIONS.remove(&session_key);
				return Err(io::Error::new(
					io::ErrorKind::ConnectionReset,
					"Failed to send to upstream",
				));
			}
			return Ok(());
		} else {
			if let Ok(addr) = session.upstream_socket.local_addr() {
				REVERSE_SESSIONS.remove(&addr);
			}
		}
	}

	if let Ok(target_ip) = target.ip.parse::<IpAddr>() {
		if let Ok(upstream_socket) = bind_upstream_socket(&target_ip).await {
			let upstream_arc = Arc::new(upstream_socket);

			if let Ok(local_addr) = upstream_arc.local_addr() {
				// Apply Connection Rate Limits
				let guard = match crate::modules::ports::tasks::GLOBAL_TRACKER.acquire(client_addr.ip()) {
					Some(g) => g,
					None => {
						log(
							LogLevel::Debug,
							&format!(
								"⚙ Rate limited UDP Flow session from {} to {}:{}",
								client_addr, target.ip, target.port
							),
						);
						return Err(io::Error::new(io::ErrorKind::Other, "Rate limited"));
					}
				};

				let new_session = Arc::new(Session {
					target: target.clone(),
					upstream_socket: upstream_arc.clone(),
					last_seen: Instant::now(),
					_guard: guard,
				});

				SESSIONS.insert(session_key.clone(), new_session.clone());
				REVERSE_SESSIONS.insert(local_addr, client_addr);

				let timeout_ms_str = if ip::is_private_ip(&target_ip) {
					getenv::get_env("UDP_TIMEOUT_LOCAL", "500".to_string())
				} else {
					getenv::get_env("UDP_TIMEOUT_REMOTE", "5000".to_string())
				};
				let timeout_ms = timeout_ms_str.parse::<u64>().unwrap_or(5000);

				spawn_reply_handler(
					upstream_arc.clone(),
					main_socket,
					Duration::from_millis(timeout_ms),
				);

				let target_addr = (target.ip.as_str(), target.port);

				// FIX: Added error handling for the initial packet send logic in Flow UDP.
				// Although send_to on UDP rarely fails instantly like TCP connect,
				// if the network is unreachable, it should trigger health downgrade.
				if let Err(e) = upstream_arc.send_to(datagram, target_addr).await {
					log(
						LogLevel::Error,
						&format!(
							"✗ Failed to send initial UDP packet to {}: {}",
							target_addr.0, e
						),
					);
					health::mark_udp_target_unhealthy(&target);
					// Cleanup
					SESSIONS.remove(&session_key);
					REVERSE_SESSIONS.remove(&local_addr);
					return Err(e);
				}

				log(
					LogLevel::Debug,
					&format!(
						"➜ Established UDP NAT mapping: {} <-> {}",
						client_addr, nat_key
					),
				);
				return Ok(());
			}
		}
	}

	Err(io::Error::new(
		io::ErrorKind::Other,
		"Failed to create UDP NAT mapping",
	))
}
