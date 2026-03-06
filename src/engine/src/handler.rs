use std::net::SocketAddr;
use std::time::Duration;

use tracing::Instrument;
use vane_primitives::connection::ConnectionGuard;
use vane_primitives::kv::KvStore;

use crate::flow::{self, FlowStep, PluginRegistry, TransportContext};

/// Per-connection parameters derived from engine config.
#[derive(Clone)]
pub struct ConnectionConfig {
    pub flow_timeout: Duration,
    pub peek_limit: usize,
}

pub async fn handle_connection(
    client: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    server_addr: SocketAddr,
    step: &FlowStep,
    registry: &PluginRegistry,
    config: &ConnectionConfig,
    _guard: ConnectionGuard,
) {
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
    let mut ctx = TransportContext::new(peer_addr, server_addr, kv, client);
    if let Some(data) = peek_data {
        ctx.set_peek_data(data);
    }

    let span = tracing::info_span!("connection", %peer_addr, %server_addr);
    let result = flow::executor::execute(step, &mut ctx, registry, config.flow_timeout)
        .instrument(span.clone())
        .await;

    match result {
        Ok(()) => tracing::info!(parent: &span, "flow completed"),
        Err(e) => tracing::warn!(parent: &span, error = %e, "flow failed"),
    }
}
