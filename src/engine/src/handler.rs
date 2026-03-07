use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::ServerConfig;
use tracing::Instrument;
use vane_primitives::connection::ConnectionGuard;
use vane_primitives::kv::KvStore;
use vane_primitives::registry::{
	ConnLayer, ConnPhase, ConnectionRegistry, ConnectionState, RegistryGuard,
};
use vane_transport::stream::ConnectionStream;
use vane_transport::tls::{TlsInfo, accept_tls};

use crate::config::{Layer, PortConfig, TerminationAction};
use crate::flow::{self, ExecutionContext, PluginRegistry, TransportContext};

/// Per-connection parameters derived from engine config.
#[derive(Clone)]
pub struct ConnectionConfig {
	pub flow_timeout: Duration,
	pub peek_limit: usize,
	pub tls_config: Option<Arc<ServerConfig>>,
	pub conn_registry: Arc<ConnectionRegistry>,
}

pub async fn handle_connection(
	client: tokio::net::TcpStream,
	peer_addr: SocketAddr,
	server_addr: SocketAddr,
	port_config: &PortConfig,
	registry: &PluginRegistry,
	config: &ConnectionConfig,
	_guard: ConnectionGuard,
) {
	let _ = client.set_nodelay(true);

	// Create KvStore first to extract conn_id before peek borrows client
	let kv = KvStore::new(&peer_addr, &server_addr, "tcp");
	let conn_id = kv.conn_uuid().to_owned();

	let started_at = Instant::now();
	let state = ConnectionState {
		id: conn_id.clone(),
		peer_addr,
		server_addr,
		layer: ConnLayer::L4,
		phase: ConnPhase::Accepted,
		protocol: None,
		tls_sni: None,
		tls_version: None,
		forward_target: None,
		started_at,
	};
	let reg_guard = config.conn_registry.register(state);

	let span = tracing::info_span!("connection", conn_id = %conn_id, %peer_addr, %server_addr);
	tracing::info!(parent: &span, "connection.accepted");

	// Peek (borrows client; TransportContext::new moves it)
	reg_guard.update_phase(ConnPhase::Detecting);
	let peek_data = match vane_transport::tcp::peek_tcp(&client, config.peek_limit).await {
		Ok(data) if !data.is_empty() => Some(data),
		Ok(_) => {
			tracing::debug!(parent: &span, "peek returned empty");
			None
		}
		Err(e) => {
			tracing::warn!(parent: &span, error = %e, "peek failed");
			None
		}
	};

	let mut ctx = TransportContext::new(peer_addr, server_addr, kv, ConnectionStream::from(client));
	if let Some(data) = peek_data {
		ctx.set_peek_data(data);
	}

	let l4_span = tracing::info_span!(parent: &span, "l4_flow");
	let result = flow::executor::execute(&port_config.l4, &mut ctx, registry, config.flow_timeout)
		.instrument(l4_span)
		.await;

	// Publish detected protocol from KV (set by ProtocolDetect middleware)
	if let Some(protocol) = ctx.kv().get("conn.detected_protocol") {
		reg_guard.set_protocol(protocol.to_owned());
		tracing::info!(parent: &span, protocol, "protocol.detected");
	}

	match result {
		Ok(TerminationAction::Finished) => {
			reg_guard.update_phase(ConnPhase::Forwarding);
			let duration_ms = started_at.elapsed().as_millis() as u64;
			tracing::info!(parent: &span, duration_ms, "connection.closed");
		}
		Ok(TerminationAction::Upgrade { target_layer }) => {
			reg_guard.update_phase(ConnPhase::TlsHandshake);
			handle_upgrade(target_layer, &mut ctx, port_config, registry, config, &span, &reg_guard)
				.await;
			let duration_ms = started_at.elapsed().as_millis() as u64;
			tracing::info!(parent: &span, duration_ms, "connection.closed");
		}
		Err(e) => {
			let duration_ms = started_at.elapsed().as_millis() as u64;
			tracing::warn!(parent: &span, error = %e, duration_ms, "connection.closed");
		}
	}
}

async fn handle_upgrade(
	target_layer: Layer,
	ctx: &mut TransportContext,
	port_config: &PortConfig,
	registry: &PluginRegistry,
	config: &ConnectionConfig,
	parent_span: &tracing::Span,
	reg_guard: &RegistryGuard,
) {
	match target_layer {
		Layer::L5 => {
			handle_l5_upgrade(ctx, port_config, registry, config, parent_span, reg_guard).await;
		}
		other => {
			tracing::info!(parent: parent_span, %other, "upgrade not yet implemented");
		}
	}
}

async fn handle_l5_upgrade(
	ctx: &mut TransportContext,
	port_config: &PortConfig,
	registry: &PluginRegistry,
	config: &ConnectionConfig,
	parent_span: &tracing::Span,
	reg_guard: &RegistryGuard,
) {
	let Some(l5_config) = &port_config.l5 else {
		tracing::warn!(parent: parent_span, "L5 upgrade requested but no l5 config");
		return;
	};

	let Some(tls_config) = &config.tls_config else {
		tracing::warn!(parent: parent_span, "L5 upgrade requested but no TLS config");
		return;
	};

	let Some(stream) = ctx.take_stream() else {
		tracing::warn!(parent: parent_span, "L5 upgrade but stream already consumed");
		return;
	};

	let Some(tcp_stream) = stream.into_tcp() else {
		tracing::warn!(parent: parent_span, "L5 upgrade but stream is not raw TCP");
		return;
	};

	// TLS handshake
	let hs_span = tracing::info_span!(parent: parent_span, "tls_handshake");
	let result =
		accept_tls(tcp_stream, tls_config, config.flow_timeout).instrument(hs_span.clone()).await;

	let (tls_stream, tls_info) = match result {
		Ok(pair) => pair,
		Err(e) => {
			tracing::warn!(parent: &hs_span, error = %e, "TLS handshake failed");
			return;
		}
	};

	reg_guard.set_tls_info(tls_info.sni.clone(), tls_info.tls_version.clone());
	reg_guard.update_layer(ConnLayer::L5);
	reg_guard.update_phase(ConnPhase::Forwarding);

	// Build L5 context with TLS metadata
	let mut kv = ctx.kv().clone();
	apply_tls_info(&mut kv, &tls_info);

	let peer_addr = ctx.peer_addr();
	let server_addr = ctx.server_addr();
	let mut l5_ctx =
		TransportContext::new(peer_addr, server_addr, kv, ConnectionStream::from(tls_stream));

	let l5_span = tracing::info_span!(parent: parent_span, "l5_flow");
	let l5_result =
		flow::executor::execute(&l5_config.flow, &mut l5_ctx, registry, config.flow_timeout)
			.instrument(l5_span.clone())
			.await;

	match l5_result {
		Ok(TerminationAction::Finished) => {
			tracing::info!(parent: &l5_span, "L5 flow completed");
		}
		Ok(TerminationAction::Upgrade { target_layer }) => {
			tracing::info!(parent: &l5_span, %target_layer, "upgrade from L5 not yet implemented");
		}
		Err(e) => {
			tracing::warn!(parent: &l5_span, error = %e, "L5 flow failed");
		}
	}
}

fn apply_tls_info(kv: &mut KvStore, info: &TlsInfo) {
	if let Some(sni) = &info.sni {
		kv.set("tls.sni".to_owned(), sni.clone());
	}
	if let Some(alpn) = &info.alpn {
		kv.set("tls.alpn".to_owned(), alpn.clone());
	}
	if let Some(version) = &info.tls_version {
		kv.set("tls.version".to_owned(), version.clone());
	}
}
