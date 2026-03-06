use std::net::SocketAddr;

use tracing::Instrument;
use vane_transport::tcp::proxy_tcp;

use crate::rule::ForwardRule;

pub async fn handle_connection(client: tokio::net::TcpStream, peer_addr: SocketAddr, rule: &ForwardRule) {
    let span = tracing::info_span!("connection", %peer_addr, upstream = %rule.upstream);
    let result = proxy_tcp(client, rule.upstream, &rule.proxy_config)
        .instrument(span.clone())
        .await;

    match result {
        Ok(()) => tracing::info!(parent: &span, "connection closed"),
        Err(e) => tracing::warn!(parent: &span, error = %e, "connection failed"),
    }
}
