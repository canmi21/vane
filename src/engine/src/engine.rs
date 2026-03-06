use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use vane_primitives::connection::ConnectionTracker;
use vane_transport::error::ListenerError;
use vane_transport::listener::{start_tcp_listener, ListenerConfig, TcpListenerHandle};

use crate::flow::{FlowTable, PluginRegistry};
use crate::handler::{ConnectionConfig, handle_connection};

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("listener failed on port {port}")]
    ListenerFailed {
        port: u16,
        #[source]
        source: ListenerError,
    },
}

#[derive(Clone)]
pub struct EngineConfig {
    pub max_connections: usize,
    pub max_connections_per_ip: usize,
    pub flow_timeout: Duration,
    pub peek_limit: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_connections: 10000,
            max_connections_per_ip: 50,
            flow_timeout: Duration::from_secs(10),
            peek_limit: 64,
        }
    }
}

pub struct Engine {
    flow_table: Arc<FlowTable>,
    registry: Arc<PluginRegistry>,
    config: EngineConfig,
    tracker: Arc<ConnectionTracker>,
    handles: Vec<TcpListenerHandle>,
}

impl Engine {
    pub fn new(flow_table: FlowTable, registry: PluginRegistry, config: EngineConfig) -> Self {
        Self {
            flow_table: Arc::new(flow_table),
            registry: Arc::new(registry),
            tracker: Arc::new(ConnectionTracker::new(
                config.max_connections,
                config.max_connections_per_ip,
            )),
            config,
            handles: Vec::new(),
        }
    }

    pub async fn start(&mut self) -> Result<(), EngineError> {
        let span = tracing::info_span!("engine");
        let guard = span.enter();
        let ports: Vec<u16> = self.flow_table.ports().collect();
        drop(guard);

        for port in ports {
            let config = ListenerConfig {
                port,
                ..Default::default()
            };

            let table = self.flow_table.clone();
            let registry = self.registry.clone();
            let tracker = self.tracker.clone();
            let conn_config = ConnectionConfig {
                flow_timeout: self.config.flow_timeout,
                peek_limit: self.config.peek_limit,
            };
            let listener_port = port;

            let handle =
                start_tcp_listener(&config, move |stream, peer_addr, server_addr| {
                    let table = table.clone();
                    let registry = registry.clone();
                    let tracker = tracker.clone();
                    let conn_config = conn_config.clone();
                    tokio::spawn(async move {
                        let Some(guard) = tracker.acquire(peer_addr.ip()) else {
                            tracing::warn!(
                                %peer_addr,
                                "connection rejected: limit exceeded"
                            );
                            return;
                        };
                        if let Some(step) = table.lookup(listener_port) {
                            handle_connection(
                                stream,
                                peer_addr,
                                server_addr,
                                step,
                                &registry,
                                &conn_config,
                                guard,
                            )
                            .await;
                        } else {
                            tracing::warn!(port = listener_port, "no flow found for port");
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
