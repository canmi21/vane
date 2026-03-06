use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::watch;
use vane_primitives::connection::ConnectionTracker;
use vane_transport::error::ListenerError;
use vane_transport::listener::{ListenerConfig, TcpListenerHandle, start_tcp_listener};

use crate::config::ConfigTable;
use crate::config::validate::ValidationError;
use crate::flow::PluginRegistry;
use crate::handler::{ConnectionConfig, handle_connection};

#[derive(Debug, Error)]
pub enum EngineError {
	#[error("listener failed on port {port}")]
	ListenerFailed {
		port: u16,
		#[source]
		source: ListenerError,
	},

	#[error("config validation failed ({} errors)", .0.len())]
	ConfigInvalid(Vec<ValidationError>),
}

pub struct Engine {
	config_tx: Arc<watch::Sender<Arc<ConfigTable>>>,
	registry: Arc<PluginRegistry>,
	tracker: Arc<ConnectionTracker>,
	handles: Vec<TcpListenerHandle>,
}

impl Engine {
	/// Create a new engine. Validates the config against the registry.
	pub fn new(config: ConfigTable, registry: PluginRegistry) -> Result<Self, EngineError> {
		config.validate(&registry).map_err(EngineError::ConfigInvalid)?;

		let tracker = Arc::new(ConnectionTracker::new(
			config.global.max_connections,
			config.global.max_connections_per_ip,
		));

		let (config_tx, _) = watch::channel(Arc::new(config));

		Ok(Self {
			config_tx: Arc::new(config_tx),
			registry: Arc::new(registry),
			tracker,
			handles: Vec::new(),
		})
	}

	pub async fn start(&mut self) -> Result<(), EngineError> {
		let config = self.config_tx.borrow().clone();
		let ports: Vec<u16> = config.ports.keys().copied().collect();

		for port in ports {
			let listener_config = ListenerConfig {
				port,
				ipv6: config.ports.get(&port).is_some_and(|p| p.listen.ipv6),
				..Default::default()
			};

			let config_tx = self.config_tx.clone();
			let registry = self.registry.clone();
			let tracker = self.tracker.clone();
			let listener_port = port;

			let handle = start_tcp_listener(&listener_config, move |stream, peer_addr, server_addr| {
				let config_tx = config_tx.clone();
				let registry = registry.clone();
				let tracker = tracker.clone();
				tokio::spawn(async move {
					let Some(guard) = tracker.acquire(peer_addr.ip()) else {
						tracing::warn!(
								%peer_addr,
								"connection rejected: limit exceeded"
						);
						return;
					};

					// Read fresh config per connection
					let config = config_tx.borrow().clone();
					let Some(port_config) = config.ports.get(&listener_port) else {
						tracing::warn!(port = listener_port, "no flow found for port");
						return;
					};

					let conn_config = ConnectionConfig {
						flow_timeout: Duration::from_millis(config.global.flow_timeout_ms),
						peek_limit: config.global.peek_limit,
					};

					handle_connection(
						stream,
						peer_addr,
						server_addr,
						port_config,
						&registry,
						&conn_config,
						guard,
					)
					.await;
				});
			})
			.await
			.map_err(|source| EngineError::ListenerFailed { port, source })?;

			tracing::info!(port, addr = %handle.local_addr(), "listener started");
			self.handles.push(handle);
		}

		Ok(())
	}

	/// Atomically swap the running config. Does NOT start/stop listeners for
	/// added/removed ports — that requires a restart for now.
	pub fn update_config(&self, config: ConfigTable) -> Result<(), EngineError> {
		config.validate(&self.registry).map_err(EngineError::ConfigInvalid)?;
		self.config_tx.send_replace(Arc::new(config));
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
