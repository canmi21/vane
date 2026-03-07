use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rustls::ServerConfig;
use thiserror::Error;
use tokio::sync::watch;
use vane_primitives::connection::ConnectionTracker;
use vane_primitives::registry::ConnectionRegistry;
use vane_transport::error::ListenerError;
use vane_transport::listener::{ListenerConfig, TcpListenerHandle, start_tcp_listener};
use vane_transport::tls::{CertStore, TlsAcceptError, build_server_config};

use crate::config::ConfigTable;
use crate::config::validate::ValidationError;
use crate::flow::PluginRegistry;
use crate::handler::{ConnectionConfig, handle_connection};

/// Minimum peek buffer for ports with TLS (L5) config, large enough to capture
/// a typical `ClientHello` (~200-2000 bytes).
const TLS_PEEK_LIMIT: usize = 4096;

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

	#[error("TLS config build failed for port {port}")]
	TlsBuildFailed {
		port: u16,
		#[source]
		source: TlsAcceptError,
	},
}

pub struct Engine {
	config_tx: Arc<watch::Sender<Arc<ConfigTable>>>,
	registry: Arc<PluginRegistry>,
	tracker: Arc<ConnectionTracker>,
	conn_registry: Arc<ConnectionRegistry>,
	tls_configs: Arc<HashMap<u16, Arc<ServerConfig>>>,
	handles: Vec<TcpListenerHandle>,
}

impl Engine {
	/// Create a new engine. Validates the config against the registry and builds
	/// TLS configs for ports with L5 configuration.
	pub fn new(
		config: ConfigTable,
		registry: PluginRegistry,
		cert_store: CertStore,
	) -> Result<Self, EngineError> {
		config.validate(&registry).map_err(EngineError::ConfigInvalid)?;

		let tracker = Arc::new(ConnectionTracker::new(
			config.global.max_connections,
			config.global.max_connections_per_ip,
		));

		let tls_configs = build_tls_configs(&config, cert_store)?;

		let (config_tx, _) = watch::channel(Arc::new(config));

		Ok(Self {
			config_tx: Arc::new(config_tx),
			registry: Arc::new(registry),
			tracker,
			conn_registry: Arc::new(ConnectionRegistry::new()),
			tls_configs: Arc::new(tls_configs),
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
			let conn_registry = self.conn_registry.clone();
			let tls_configs = self.tls_configs.clone();
			let listener_port = port;

			let handle = start_tcp_listener(&listener_config, move |stream, peer_addr, server_addr| {
				let config_tx = config_tx.clone();
				let registry = registry.clone();
				let tracker = tracker.clone();
				let conn_registry = conn_registry.clone();
				let tls_configs = tls_configs.clone();
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

					let peek_limit = if port_config.l5.is_some() {
						config.global.peek_limit.max(TLS_PEEK_LIMIT)
					} else {
						config.global.peek_limit
					};

					let conn_config = ConnectionConfig {
						flow_timeout: Duration::from_millis(config.global.flow_timeout_ms),
						peek_limit,
						tls_config: tls_configs.get(&listener_port).cloned(),
						conn_registry,
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

	pub fn conn_registry(&self) -> &ConnectionRegistry {
		&self.conn_registry
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

fn build_tls_configs(
	config: &ConfigTable,
	cert_store: CertStore,
) -> Result<HashMap<u16, Arc<ServerConfig>>, EngineError> {
	let mut tls_configs = HashMap::new();
	let store = Arc::new(cert_store);

	for (&port, port_config) in &config.ports {
		if let Some(l5) = &port_config.l5 {
			let server_config = build_server_config(store.clone(), &l5.alpn)
				.map_err(|source| EngineError::TlsBuildFailed { port, source })?;
			tls_configs.insert(port, server_config);
		}
	}

	Ok(tls_configs)
}
