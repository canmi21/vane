use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use vane_primitives::connection::ConnectionGuard;
use vane_primitives::model::ResolvedTarget;
use vane_primitives::registry::{ConnPhase, ConnectionRegistry, ConnectionState};
use vane_transport::tcp::{ProxyConfig, proxy_tcp};

use crate::config::TargetAddr;

/// Per-connection parameters derived from engine config.
#[derive(Clone)]
pub struct ConnectionConfig {
	pub proxy_config: ProxyConfig,
	pub conn_registry: Arc<ConnectionRegistry>,
}

pub async fn handle_connection(
	client: tokio::net::TcpStream,
	peer_addr: SocketAddr,
	server_addr: SocketAddr,
	target: &TargetAddr,
	config: &ConnectionConfig,
	_guard: ConnectionGuard,
) {
	let _ = client.set_nodelay(true);

	let conn_id = format!("{:032x}", fastrand::u128(..));
	let started_at = Instant::now();

	let state = ConnectionState {
		id: conn_id.clone(),
		peer_addr,
		server_addr,
		phase: ConnPhase::Accepted,
		forward_target: None,
		started_at,
	};
	let reg_guard = config.conn_registry.register(state);

	let span = tracing::info_span!("connection", conn_id = %conn_id, %peer_addr, %server_addr);
	tracing::info!(parent: &span, "connection.accepted");

	let target_addr: SocketAddr = match format!("{}:{}", target.ip, target.port).parse() {
		Ok(addr) => addr,
		Err(e) => {
			tracing::warn!(parent: &span, error = %e, "invalid target address");
			return;
		}
	};

	reg_guard.set_forward_target(target_addr);
	reg_guard.update_phase(ConnPhase::Forwarding);

	let resolved = ResolvedTarget { addr: target_addr };
	let forward_span = tracing::info_span!(parent: &span, "forward", %target_addr);

	match proxy_tcp(client, &resolved, &config.proxy_config).instrument(forward_span).await {
		Ok(()) => {
			let duration_ms = started_at.elapsed().as_millis() as u64;
			tracing::info!(parent: &span, duration_ms, "connection.closed");
		}
		Err(e) => {
			let duration_ms = started_at.elapsed().as_millis() as u64;
			tracing::warn!(parent: &span, error = %e, duration_ms, "connection.closed");
		}
	}
}

use tracing::Instrument;
