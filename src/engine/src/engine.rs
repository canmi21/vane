use std::sync::Arc;

use thiserror::Error;
use vane_primitives::connection::ConnectionTracker;
use vane_transport::error::ListenerError;
use vane_transport::listener::{start_tcp_listener, ListenerConfig, TcpListenerHandle};

use crate::handler::handle_connection;
use crate::rule::RouteTable;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("listener failed on port {port}")]
    ListenerFailed {
        port: u16,
        #[source]
        source: ListenerError,
    },
}

#[derive(Clone, Copy)]
pub struct EngineConfig {
    pub max_connections: usize,
    pub max_connections_per_ip: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_connections: 10000,
            max_connections_per_ip: 50,
        }
    }
}

pub struct Engine {
    route_table: Arc<RouteTable>,
    tracker: Arc<ConnectionTracker>,
    handles: Vec<TcpListenerHandle>,
}

impl Engine {
    pub fn new(route_table: RouteTable, config: EngineConfig) -> Self {
        Self {
            route_table: Arc::new(route_table),
            tracker: Arc::new(ConnectionTracker::new(
                config.max_connections,
                config.max_connections_per_ip,
            )),
            handles: Vec::new(),
        }
    }

    pub async fn start(&mut self) -> Result<(), EngineError> {
        let span = tracing::info_span!("engine");
        let guard = span.enter();
        let ports: Vec<u16> = self.route_table.ports().collect();
        drop(guard);

        for port in ports {
            let config = ListenerConfig {
                port,
                ..Default::default()
            };

            let table = self.route_table.clone();
            let tracker = self.tracker.clone();
            let listener_port = port;

            let handle =
                start_tcp_listener(&config, move |stream, peer_addr, server_addr| {
                    let table = table.clone();
                    let tracker = tracker.clone();
                    tokio::spawn(async move {
                        let Some(guard) = tracker.acquire(peer_addr.ip()) else {
                            tracing::warn!(
                                %peer_addr,
                                "connection rejected: limit exceeded"
                            );
                            return;
                        };
                        if let Some(rule) = table.lookup(listener_port) {
                            handle_connection(stream, peer_addr, server_addr, rule, guard).await;
                        } else {
                            tracing::warn!(port = listener_port, "no rule found for port");
                        }
                    });
                })
                .await
                .map_err(|source| EngineError::ListenerFailed { port, source })?;

            tracing::info!(port, addr = %handle.local_addr(), "listener started");
            self.handles.push(handle);
        }

        Ok(())
    }

    pub fn listeners(&self) -> &[TcpListenerHandle] {
        &self.handles
    }

    pub fn shutdown(&self) {
        for handle in &self.handles {
            handle.shutdown();
        }
    }

    pub async fn join(self) {
        for handle in self.handles {
            let _ = handle.join().await;
        }
    }
}
