use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::ServerConfig;
use tracing::Instrument;
use vane_primitives::connection::ConnectionGuard;
use vane_primitives::kv::KvStore;
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

	// Peek before creating context (peek borrows, new() moves)
	let peek_data = match vane_transport::tcp::peek_tcp(&client, config.peek_limit).await {
		Ok(data) if !data.is_empty() => Some(data),
		Ok(_) => {
			tracing::debug!("peek returned empty");
			None
		}
		Err(e) => {
			tracing::warn!(error = %e, "peek failed");
			None
		}
	};

	let kv = KvStore::new(&peer_addr, &server_addr, "tcp");
	let mut ctx = TransportContext::new(peer_addr, server_addr, kv, ConnectionStream::from(client));
	if let Some(data) = peek_data {
		ctx.set_peek_data(data);
	}

	let span = tracing::info_span!("connection", %peer_addr, %server_addr);
	let result = flow::executor::execute(&port_config.l4, &mut ctx, registry, config.flow_timeout)
		.instrument(span.clone())
		.await;

	match result {
		Ok(TerminationAction::Finished) => {
			tracing::info!(parent: &span, "flow completed");
		}
		Ok(TerminationAction::Upgrade { target_layer }) => {
			handle_upgrade(target_layer, &mut ctx, port_config, registry, config, &span).await;
		}
		Err(e) => tracing::warn!(parent: &span, error = %e, "flow failed"),
	}
}

async fn handle_upgrade(
	target_layer: Layer,
	ctx: &mut TransportContext,
	port_config: &PortConfig,
	registry: &PluginRegistry,
	config: &ConnectionConfig,
	parent_span: &tracing::Span,
) {
	match target_layer {
		Layer::L5 => {
			handle_l5_upgrade(ctx, port_config, registry, config, parent_span).await;
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

	let span = tracing::info_span!(parent: parent_span, "l5_upgrade");
	let result = async {
		let (tls_stream, tls_info) = accept_tls(tcp_stream, tls_config, config.flow_timeout).await?;
		Ok::<_, vane_transport::tls::TlsAcceptError>((tls_stream, tls_info))
	}
	.instrument(span.clone())
	.await;

	let (tls_stream, tls_info) = match result {
		Ok(pair) => pair,
		Err(e) => {
			tracing::warn!(parent: &span, error = %e, "TLS handshake failed");
			return;
		}
	};

	// Build L5 context with TLS metadata
	let mut kv = ctx.kv().clone();
	apply_tls_info(&mut kv, &tls_info);

	let peer_addr = ctx.peer_addr();
	let server_addr = ctx.server_addr();
	let mut l5_ctx =
		TransportContext::new(peer_addr, server_addr, kv, ConnectionStream::from(tls_stream));

	let l5_result =
		flow::executor::execute(&l5_config.flow, &mut l5_ctx, registry, config.flow_timeout)
			.instrument(span.clone())
			.await;

	match l5_result {
		Ok(TerminationAction::Finished) => {
			tracing::info!(parent: &span, "L5 flow completed");
		}
		Ok(TerminationAction::Upgrade { target_layer }) => {
			tracing::info!(parent: &span, %target_layer, "upgrade from L5 not yet implemented");
		}
		Err(e) => {
			tracing::warn!(parent: &span, error = %e, "L5 flow failed");
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
