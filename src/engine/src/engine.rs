use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
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

	#[error("port {port} is already running")]
	PortAlreadyRunning { port: u16 },

	#[error("port {port} is not running")]
	PortNotRunning { port: u16 },

	#[error("port {port} not found in current config")]
	PortNotConfigured { port: u16 },
}

pub struct Engine {
	config_tx: Arc<watch::Sender<Arc<ConfigTable>>>,
	registry: Arc<PluginRegistry>,
	tracker: Arc<ConnectionTracker>,
	conn_registry: Arc<ConnectionRegistry>,
	cert_store: Arc<CertStore>,
	tls_configs: Arc<DashMap<u16, Arc<ServerConfig>>>,
	handles: Arc<DashMap<u16, TcpListenerHandle>>,
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

		let cert_store = Arc::new(cert_store);
		let tls_configs = build_tls_configs(&config, &cert_store)?;

		let (config_tx, _) = watch::channel(Arc::new(config));

		Ok(Self {
			config_tx: Arc::new(config_tx),
			registry: Arc::new(registry),
			tracker,
			conn_registry: Arc::new(ConnectionRegistry::new()),
			cert_store,
			tls_configs: Arc::new(tls_configs),
			handles: Arc::new(DashMap::new()),
		})
	}

	/// Start listeners on all configured ports.
	pub async fn start(&self) -> Result<(), EngineError> {
		let config = self.config_tx.borrow().clone();
		let ports: Vec<u16> = config.ports.keys().copied().collect();
		for port in ports {
			self.start_port(port).await?;
		}
		Ok(())
	}

	/// Start a single listener for the given config port.
	pub async fn start_port(&self, port: u16) -> Result<(), EngineError> {
		if self.handles.contains_key(&port) {
			return Err(EngineError::PortAlreadyRunning { port });
		}

		let config = self.config_tx.borrow().clone();
		let Some(port_config) = config.ports.get(&port) else {
			return Err(EngineError::PortNotConfigured { port });
		};

		// Build TLS config on demand if L5 is present but tls_configs lacks it
		if let Some(l5) = &port_config.l5
			&& !self.tls_configs.contains_key(&port)
		{
			let server_config = build_server_config(self.cert_store.clone(), &l5.alpn)
				.map_err(|source| EngineError::TlsBuildFailed { port, source })?;
			self.tls_configs.insert(port, server_config);
		}

		let listener_config =
			ListenerConfig { port, ipv6: port_config.listen.ipv6, ..Default::default() };

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
					tls_config: tls_configs.get(&listener_port).map(|r| r.clone()),
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
		self.handles.insert(port, handle);

		Ok(())
	}

	/// Stop the listener on the given config port (graceful shutdown).
	pub fn stop_port(&self, port: u16) -> Result<(), EngineError> {
		let Some((_, handle)) = self.handles.remove(&port) else {
			return Err(EngineError::PortNotRunning { port });
		};
		handle.shutdown();
		self.tls_configs.remove(&port);

		// Join the listener task in the background so resources are cleaned up
		tokio::spawn(async move {
			let _ = handle.join().await;
		});

		tracing::info!(port, "listener stopped");
		Ok(())
	}

	/// Atomically swap the running config and reconcile listeners.
	///
	/// - Ports removed from the new config are shut down.
	/// - Ports added in the new config start listening.
	/// - Ports kept but with changed L5 config get their TLS rebuilt.
	/// - Ports kept with unchanged config rely on watch channel (next connection reads new config).
	pub async fn update_config(&self, config: ConfigTable) -> Result<(), EngineError> {
		config.validate(&self.registry).map_err(EngineError::ConfigInvalid)?;

		let old_config = self.config_tx.borrow().clone();
		let old_ports: HashSet<u16> = old_config.ports.keys().copied().collect();
		let new_ports: HashSet<u16> = config.ports.keys().copied().collect();

		let to_stop: Vec<u16> = old_ports.difference(&new_ports).copied().collect();
		let to_start: Vec<u16> = new_ports.difference(&old_ports).copied().collect();

		// Stop removed ports before swapping config
		for &port in &to_stop {
			if let Err(e) = self.stop_port(port) {
				tracing::warn!(port, error = %e, "failed to stop port during config update");
			}
		}

		// Rebuild TLS configs for kept ports whose L5 config changed
		for &port in new_ports.intersection(&old_ports) {
			let old_l5 = old_config.ports.get(&port).and_then(|p| p.l5.as_ref());
			let new_l5 = config.ports.get(&port).and_then(|p| p.l5.as_ref());

			if old_l5 != new_l5 {
				self.tls_configs.remove(&port);
				if let Some(l5) = new_l5 {
					let server_config = build_server_config(self.cert_store.clone(), &l5.alpn)
						.map_err(|source| EngineError::TlsBuildFailed { port, source })?;
					self.tls_configs.insert(port, server_config);
				}
			}
		}

		// Swap config — new connections on kept ports will read the new config
		self.config_tx.send_replace(Arc::new(config));

		// Start newly added ports
		let mut first_error = None;
		for &port in &to_start {
			if let Err(e) = self.start_port(port).await {
				tracing::error!(port, error = %e, "failed to start port during config update");
				if first_error.is_none() {
					first_error = Some(e);
				}
			}
		}

		if let Some(e) = first_error {
			return Err(e);
		}

		Ok(())
	}

	pub fn current_config(&self) -> Arc<ConfigTable> {
		self.config_tx.borrow().clone()
	}

	pub fn conn_registry(&self) -> &ConnectionRegistry {
		&self.conn_registry
	}

	/// Get the actual listening address for a config port.
	pub fn listener_addr(&self, config_port: u16) -> Option<SocketAddr> {
		self.handles.get(&config_port).map(|r| r.local_addr())
	}

	/// Get all currently listening addresses with their config ports.
	pub fn listener_addrs(&self) -> Vec<(u16, SocketAddr)> {
		self.handles.iter().map(|r| (*r.key(), r.local_addr())).collect()
	}

	pub fn shutdown(&self) {
		for entry in self.handles.iter() {
			entry.value().shutdown();
		}
	}

	pub async fn join(self) {
		let keys: Vec<u16> = self.handles.iter().map(|r| *r.key()).collect();
		for key in keys {
			if let Some((_, handle)) = self.handles.remove(&key) {
				let _ = handle.join().await;
			}
		}
	}
}

fn build_tls_configs(
	config: &ConfigTable,
	cert_store: &Arc<CertStore>,
) -> Result<DashMap<u16, Arc<ServerConfig>>, EngineError> {
	let tls_configs = DashMap::new();

	for (&port, port_config) in &config.ports {
		if let Some(l5) = &port_config.l5 {
			let server_config = build_server_config(cert_store.clone(), &l5.alpn)
				.map_err(|source| EngineError::TlsBuildFailed { port, source })?;
			tls_configs.insert(port, server_config);
		}
	}

	Ok(tls_configs)
}
