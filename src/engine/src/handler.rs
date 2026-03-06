use std::net::SocketAddr;

use tracing::Instrument;
use vane_primitives::connection::ConnectionGuard;
use vane_primitives::kv::KvStore;
use vane_primitives::model::{Forward, ResolvedTarget, Strategy, Target};
use vane_transport::tcp::proxy_tcp;

use crate::rule::PortRule;

fn resolve_target(target: &Target) -> Option<ResolvedTarget> {
    match target {
        Target::Ip { ip, port } => Some(ResolvedTarget {
            addr: SocketAddr::new(*ip, *port),
        }),
        Target::Domain { domain, .. } => {
            tracing::warn!(%domain, "domain target resolution not yet supported");
            None
        }
    }
}

fn select_target(forward: &Forward) -> Option<&Target> {
    if forward.targets.is_empty() {
        return None;
    }
    match forward.strategy {
        Strategy::Random => {
            let idx = fastrand::usize(..forward.targets.len());
            Some(&forward.targets[idx])
        }
        Strategy::Serial | Strategy::Fastest => {
            tracing::warn!(strategy = ?forward.strategy, "strategy not yet implemented, using first target");
            Some(&forward.targets[0])
        }
    }
}

pub async fn handle_connection(
    client: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    server_addr: SocketAddr,
    rule: &PortRule,
    _guard: ConnectionGuard,
) {
    let _kv = KvStore::new(&peer_addr, &server_addr, "tcp");

    let target = match select_target(&rule.forward) {
        Some(t) => t,
        None => {
            tracing::warn!(%peer_addr, "no targets configured");
            return;
        }
    };

    let resolved = match resolve_target(target) {
        Some(r) => r,
        None => return,
    };

    let span = tracing::info_span!("connection", %peer_addr, upstream = %resolved.addr);
    let result = proxy_tcp(client, &resolved, &rule.proxy_config)
        .instrument(span.clone())
        .await;

    match result {
        Ok(()) => tracing::info!(parent: &span, "connection closed"),
        Err(e) => tracing::warn!(parent: &span, error = %e, "connection failed"),
    }
}
