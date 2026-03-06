use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::error::ListenerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerState {
	Active,
	Draining,
}

#[derive(Debug, Clone)]
pub struct ListenerConfig {
	pub port: u16,
	pub ipv6: bool,
	pub bind_retries: u32,
	pub bind_retry_interval: Duration,
}

impl Default for ListenerConfig {
	fn default() -> Self {
		Self {
			port: 0,
			ipv6: false,
			bind_retries: 5,
			bind_retry_interval: Duration::from_millis(100),
		}
	}
}

impl ListenerConfig {
	fn bind_addr(&self) -> SocketAddr {
		if self.ipv6 {
			SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), self.port)
		} else {
			SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), self.port)
		}
	}
}

pub struct TcpListenerHandle {
	state_tx: watch::Sender<ListenerState>,
	local_addr: SocketAddr,
	join_handle: JoinHandle<()>,
}

impl std::fmt::Debug for TcpListenerHandle {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("TcpListenerHandle")
			.field("local_addr", &self.local_addr)
			.field("state", &self.state())
			.finish()
	}
}

impl TcpListenerHandle {
	pub fn shutdown(&self) {
		let _ = self.state_tx.send(ListenerState::Draining);
	}

	pub fn state(&self) -> ListenerState {
		*self.state_tx.borrow()
	}

	pub fn local_addr(&self) -> SocketAddr {
		self.local_addr
	}

	pub async fn join(self) -> Result<(), tokio::task::JoinError> {
		self.join_handle.await
	}
}

async fn bind_with_retry(
	addr: SocketAddr,
	retries: u32,
	interval: Duration,
) -> Result<TcpListener, ListenerError> {
	let mut last_err = None;
	for attempt in 0..retries {
		match TcpListener::bind(addr).await {
			Ok(listener) => return Ok(listener),
			Err(e) => {
				last_err = Some(e);
				if attempt + 1 < retries {
					tokio::time::sleep(interval).await;
				}
			}
		}
	}
	Err(ListenerError::BindFailed {
		addr,
		attempts: retries,
		source: last_err.unwrap(),
	})
}

pub async fn start_tcp_listener<F>(
	config: &ListenerConfig,
	on_connection: F,
) -> Result<TcpListenerHandle, ListenerError>
where
	F: Fn(TcpStream, SocketAddr) + Send + Sync + 'static,
{
	let addr = config.bind_addr();
	let listener = bind_with_retry(addr, config.bind_retries, config.bind_retry_interval).await?;
	let local_addr = listener.local_addr().unwrap_or(addr);

	let (state_tx, mut state_rx) = watch::channel(ListenerState::Active);

	let span = tracing::info_span!("tcp_listener", %local_addr);
	tracing::info!(parent: &span, "tcp listener started");

	let join_handle = tokio::spawn(
		async move {
			loop {
				tokio::select! {
					result = listener.accept() => {
						match result {
							Ok((stream, peer_addr)) => {
								let _conn = tracing::debug_span!("tcp_conn", %peer_addr).entered();
								tracing::debug!("accepted connection");
								on_connection(stream, peer_addr);
							}
							Err(e) => {
								tracing::warn!(error = %e, "accept failed");
							}
						}
					}
					_ = state_rx.changed() => {
						tracing::info!("shutting down");
						break;
					}
				}
			}
		}
		.instrument(span),
	);

	Ok(TcpListenerHandle {
		state_tx,
		local_addr,
		join_handle,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use tokio::io::AsyncWriteExt;

	#[tokio::test]
	async fn test_accept_connection() {
		let (tx, mut rx) = tokio::sync::mpsc::channel::<SocketAddr>(16);
		let config = ListenerConfig {
			port: 0,
			..Default::default()
		};

		let handle = start_tcp_listener(&config, move |_stream, addr| {
			let _ = tx.try_send(addr);
		})
		.await
		.unwrap();

		let local_addr = handle.local_addr();
		let conn = TcpStream::connect(local_addr).await.unwrap();
		let reported_peer = rx.recv().await.unwrap();

		assert_eq!(reported_peer, conn.local_addr().unwrap());

		handle.shutdown();
		handle.join().await.unwrap();
	}

	#[tokio::test]
	async fn test_graceful_shutdown() {
		let config = ListenerConfig {
			port: 0,
			..Default::default()
		};

		let handle = start_tcp_listener(&config, |_, _| {}).await.unwrap();
		let local_addr = handle.local_addr();

		// Can connect before shutdown
		let mut conn = TcpStream::connect(local_addr).await.unwrap();
		conn.shutdown().await.ok();
		drop(conn);

		handle.shutdown();
		assert_eq!(handle.state(), ListenerState::Draining);
		handle.join().await.unwrap();

		// After shutdown, new connections should fail
		let result = tokio::time::timeout(
			Duration::from_millis(100),
			TcpStream::connect(local_addr),
		)
		.await;

		assert!(
			result.is_err() || result.unwrap().is_err(),
			"connection should fail after shutdown"
		);
	}

	#[tokio::test]
	async fn test_bind_failure() {
		// Occupy a port
		let occupied =
			TcpListener::bind(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0))
				.await
				.unwrap();
		let port = occupied.local_addr().unwrap().port();

		let config = ListenerConfig {
			port,
			ipv6: false,
			bind_retries: 1,
			bind_retry_interval: Duration::from_millis(10),
		};

		let result = start_tcp_listener(&config, |_, _| {}).await;
		assert!(
			matches!(result, Err(ListenerError::BindFailed { .. })),
			"expected BindFailed, got {result:?}"
		);
	}
}
