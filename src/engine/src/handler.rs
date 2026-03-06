use std::net::SocketAddr;
use std::time::Duration;

use tracing::Instrument;
use vane_primitives::connection::ConnectionGuard;
use vane_primitives::kv::KvStore;

use crate::flow::{self, FlowStep, PluginRegistry, TransportContext};

pub async fn handle_connection(
    client: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    server_addr: SocketAddr,
    step: &FlowStep,
    registry: &PluginRegistry,
    flow_timeout: Duration,
    _guard: ConnectionGuard,
) {
    let kv = KvStore::new(&peer_addr, &server_addr, "tcp");
    let mut ctx = TransportContext::new(peer_addr, server_addr, kv, client);

    let span = tracing::info_span!("connection", %peer_addr, %server_addr);
    let result = flow::executor::execute(step, &mut ctx, registry, flow_timeout)
        .instrument(span.clone())
        .await;

    match result {
        Ok(()) => tracing::info!(parent: &span, "flow completed"),
        Err(e) => tracing::warn!(parent: &span, error = %e, "flow failed"),
    }
}
