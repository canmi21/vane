use std::sync::Arc;

use thiserror::Error;
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

pub struct Engine {
    route_table: Arc<RouteTable>,
    handles: Vec<TcpListenerHandle>,
}

impl Engine {
    pub fn new(route_table: RouteTable) -> Self {
        Self {
            route_table: Arc::new(route_table),
            handles: Vec::new(),
        }
    }

    pub async fn start(&mut self) -> Result<(), EngineError> {
        let _span = tracing::info_span!("engine").entered();
        let ports: Vec<u16> = self.route_table.ports().collect();

        for port in ports {
            let config = ListenerConfig {
                port,
                ..Default::default()
            };

            let table = self.route_table.clone();
            let listener_port = port;

            let handle = start_tcp_listener(&config, move |stream, peer_addr| {
                let table = table.clone();
                tokio::spawn(async move {
                    if let Some(rule) = table.lookup(listener_port) {
                        handle_connection(stream, peer_addr, rule).await;
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
