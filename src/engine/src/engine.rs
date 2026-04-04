use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use dashmap::DashMap;
use thiserror::Error;
use tokio::sync::watch;
use vane_primitives::connection::ConnectionTracker;
use vane_primitives::registry::ConnectionRegistry;
use vane_transport::error::ListenerError;
use vane_transport::listener::{ListenerConfig, TcpListenerHandle, start_tcp_listener};
use vane_transport::tcp::ProxyConfig;

use crate::config::listener::SingleProtocol;
use crate::config::validate::ValidationError;
use crate::config::{CompiledListener, ConfigTable};
use crate::handler::{ConnectionConfig, handle_connection};

#[derive(Debug, Error)]
pub enum EngineError {
	#[error("listener failed on {addr}")]
	ListenerFailed {
		addr: SocketAddr,
		#[source]
		source: ListenerError,
	},

	#[error("config validation failed ({} errors)", .0.len())]
	ConfigInvalid(Vec<ValidationError>),

	#[error("listener {addr} is already running")]
	ListenerAlreadyRunning { addr: SocketAddr },

	#[error("listener {addr} is not running")]
	ListenerNotRunning { addr: SocketAddr },
}

/// Key type for the listener handle map: config-level bind address + port.
type ListenerKey = SocketAddr;

pub struct Engine {
	config_tx: Arc<watch::Sender<Arc<ConfigTable>>>,
	tracker: Arc<ConnectionTracker>,
	conn_registry: Arc<ConnectionRegistry>,
	handles: Arc<DashMap<ListenerKey, TcpListenerHandle>>,
}

impl Engine {
	/// Create a new engine with validated config.
	pub fn new(config: ConfigTable) -> Result<Self, EngineError> {
		config.validate().map_err(EngineError::ConfigInvalid)?;

		let tracker = Arc::new(ConnectionTracker::new(
			config.global.max_connections,
			config.global.max_connections_per_ip,
		));

		let (config_tx, _) = watch::channel(Arc::new(config));

		Ok(Self {
			config_tx: Arc::new(config_tx),
			tracker,
			conn_registry: Arc::new(ConnectionRegistry::new()),
			handles: Arc::new(DashMap::new()),
		})
	}

	/// Start listeners for all configured TCP entries.
	pub async fn start(&self) -> Result<(), EngineError> {
		let config = self.config_tx.borrow().clone();
		for entry in &config.listeners {
			if entry.protocol == SingleProtocol::Tcp {
				self.start_listener(entry).await?;
			}
		}
		Ok(())
	}

	/// Start a single TCP listener for a compiled entry.
	async fn start_listener(&self, entry: &CompiledListener) -> Result<(), EngineError> {
		let bind_addr: SocketAddr = format!("{}:{}", entry.bind, entry.port).parse().map_err(|_| {
			EngineError::ListenerFailed {
				addr: SocketAddr::from(([0, 0, 0, 0], entry.port)),
				source: ListenerError::BindFailed {
					addr: SocketAddr::from(([0, 0, 0, 0], entry.port)),
					attempts: 0,
					source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad bind address"),
				},
			}
		})?;

		if self.handles.contains_key(&bind_addr) {
			return Err(EngineError::ListenerAlreadyRunning { addr: bind_addr });
		}

		let listener_config =
			ListenerConfig { port: entry.port, ipv6: bind_addr.is_ipv6(), ..Default::default() };

		let config_tx = self.config_tx.clone();
		let tracker = self.tracker.clone();
		let conn_registry = self.conn_registry.clone();

		let handle = start_tcp_listener(&listener_config, move |stream, peer_addr, server_addr| {
			let config_tx = config_tx.clone();
			let tracker = tracker.clone();
			let conn_registry = conn_registry.clone();
			tokio::spawn(async move {
				let Some(guard) = tracker.acquire(peer_addr.ip()) else {
					tracing::warn!(%peer_addr, "connection rejected: limit exceeded");
					return;
				};

				let config = config_tx.borrow().clone();
				let Some(target) = &config.target else {
					tracing::warn!("no forward target configured, dropping connection");
					return;
				};

				let conn_config = ConnectionConfig { proxy_config: ProxyConfig::default(), conn_registry };

				handle_connection(stream, peer_addr, server_addr, target, &conn_config, guard).await;
			});
		})
		.await
		.map_err(|source| EngineError::ListenerFailed { addr: bind_addr, source })?;

		tracing::info!(%bind_addr, actual = %handle.local_addr(), "listener started");
		self.handles.insert(bind_addr, handle);

		Ok(())
	}

	/// Stop a listener by its config-level address.
	pub fn stop_listener(&self, addr: &SocketAddr) -> Result<(), EngineError> {
		let Some((_, handle)) = self.handles.remove(addr) else {
			return Err(EngineError::ListenerNotRunning { addr: *addr });
		};
		handle.shutdown();
		tokio::spawn(async move {
			let _ = handle.join().await;
		});
		tracing::info!(%addr, "listener stopped");
		Ok(())
	}

	/// Atomically swap config and reconcile listeners.
	pub async fn update_config(&self, config: ConfigTable) -> Result<(), EngineError> {
		config.validate().map_err(EngineError::ConfigInvalid)?;

		let old_config = self.config_tx.borrow().clone();

		let old_tcp = listener_addrs_set(&old_config.listeners);
		let new_tcp = listener_addrs_set(&config.listeners);

		let to_stop: Vec<SocketAddr> = old_tcp.difference(&new_tcp).copied().collect();

		// Collect new entries to start (clone to avoid borrowing config)
		let to_start: Vec<CompiledListener> = config
			.listeners
			.iter()
			.filter(|c| c.protocol == SingleProtocol::Tcp)
			.filter(|c| {
				format!("{}:{}", c.bind, c.port)
					.parse::<SocketAddr>()
					.is_ok_and(|addr| !old_tcp.contains(&addr))
			})
			.cloned()
			.collect();

		for addr in &to_stop {
			if let Err(e) = self.stop_listener(addr) {
				tracing::warn!(%addr, error = %e, "failed to stop listener during config update");
			}
		}

		self.config_tx.send_replace(Arc::new(config));

		let mut first_error = None;
		for entry in &to_start {
			if let Err(e) = self.start_listener(entry).await {
				tracing::error!(error = %e, "failed to start listener during config update");
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

	/// Get the actual bound address for a config-level listener address.
	pub fn listener_addr(&self, config_addr: SocketAddr) -> Option<SocketAddr> {
		self.handles.get(&config_addr).map(|r| r.local_addr())
	}

	/// Get all running listeners as (config addr, actual addr) pairs.
	pub fn listener_addrs(&self) -> Vec<(SocketAddr, SocketAddr)> {
		self.handles.iter().map(|r| (*r.key(), r.local_addr())).collect()
	}

	pub fn shutdown(&self) {
		for entry in self.handles.iter() {
			entry.value().shutdown();
		}
	}

	pub async fn join(self) {
		let keys: Vec<ListenerKey> = self.handles.iter().map(|r| *r.key()).collect();
		for key in keys {
			if let Some((_, handle)) = self.handles.remove(&key) {
				let _ = handle.join().await;
			}
		}
	}
}

fn listener_addrs_set(listeners: &[CompiledListener]) -> HashSet<SocketAddr> {
	listeners
		.iter()
		.filter(|c| c.protocol == SingleProtocol::Tcp)
		.filter_map(|c| format!("{}:{}", c.bind, c.port).parse().ok())
		.collect()
}
