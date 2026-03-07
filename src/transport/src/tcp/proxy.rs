use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tracing::Instrument;
use vane_primitives::model::ResolvedTarget;

use crate::error::{ProxyError, TransferDirection};
use crate::tcp::watchdog::{IdleWatchdog, now_millis};

#[derive(Debug, Clone)]
pub struct ProxyConfig {
	pub connect_timeout: Duration,
	pub idle_timeout: Duration,
	pub watchdog_poll_interval: Duration,
}

impl Default for ProxyConfig {
	fn default() -> Self {
		Self {
			connect_timeout: Duration::from_secs(5),
			idle_timeout: Duration::from_secs(10),
			watchdog_poll_interval: Duration::from_secs(1),
		}
	}
}

/// Forward traffic bidirectionally between a client stream and an upstream TCP target.
///
/// Caller is responsible for setting TCP options (e.g. `set_nodelay`) on the client
/// stream before calling, since generic `S` may not expose TCP-specific methods.
pub async fn proxy_tcp<S: AsyncRead + AsyncWrite + Unpin>(
	client: S,
	target: &ResolvedTarget,
	config: &ProxyConfig,
) -> Result<(), ProxyError> {
	let upstream_addr = target.addr;
	let span = tracing::info_span!("forward", %upstream_addr);
	async {
		tracing::debug!("connection.forwarding");

		let upstream =
			match tokio::time::timeout(config.connect_timeout, TcpStream::connect(upstream_addr)).await {
				Ok(Ok(stream)) => stream,
				Ok(Err(e)) => {
					return Err(ProxyError::ConnectFailed { addr: upstream_addr, source: e });
				}
				Err(_) => {
					return Err(ProxyError::ConnectTimeout {
						addr: upstream_addr,
						timeout_secs: config.connect_timeout.as_secs(),
					});
				}
			};

		let _ = upstream.set_nodelay(true);

		let last_activity = Arc::new(AtomicU64::new(now_millis()));

		let mut client_wrapped = IdleWatchdog::new(client, last_activity.clone());
		let mut upstream_wrapped = IdleWatchdog::new(upstream, last_activity.clone());

		let (mut cr, mut cw) = tokio::io::split(&mut client_wrapped);
		let (mut ur, mut uw) = tokio::io::split(&mut upstream_wrapped);

		let client_to_server = tokio::io::copy(&mut cr, &mut uw);
		let server_to_client = tokio::io::copy(&mut ur, &mut cw);

		let idle_millis = config.idle_timeout.as_millis() as u64;
		let poll_interval = config.watchdog_poll_interval;
		let activity = last_activity.clone();
		let watchdog = async move {
			loop {
				tokio::time::sleep(poll_interval).await;
				let last = activity.load(Ordering::Relaxed);
				if now_millis() - last >= idle_millis {
					break;
				}
			}
		};

		tokio::select! {
			res = client_to_server => match res {
				Ok(_) => { tracing::debug!("forwarding finished"); Ok(()) }
				Err(e) => Err(ProxyError::TransferFailed {
					direction: TransferDirection::ClientToServer,
					source: e,
				}),
			},
			res = server_to_client => match res {
				Ok(_) => { tracing::debug!("forwarding finished"); Ok(()) }
				Err(e) => Err(ProxyError::TransferFailed {
					direction: TransferDirection::ServerToClient,
					source: e,
				}),
			},
			() = watchdog => {
				let idle_secs = config.idle_timeout.as_secs();
				tracing::warn!(idle_secs, "idle timeout triggered");
				Err(ProxyError::IdleTimeout { idle_secs })
			}
		}
	}
	.instrument(span)
	.await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	use tokio::net::TcpListener;
	use vane_test_utils::echo::EchoServer;

	#[tokio::test]
	async fn test_bidirectional_transfer() {
		let echo = EchoServer::start().await;

		let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let proxy_addr = proxy_listener.local_addr().unwrap();

		let target = ResolvedTarget { addr: echo.addr() };
		let config = ProxyConfig {
			connect_timeout: Duration::from_secs(2),
			idle_timeout: Duration::from_secs(5),
			watchdog_poll_interval: Duration::from_millis(100),
		};

		let proxy_task = tokio::spawn(async move {
			let (client_stream, _) = proxy_listener.accept().await.unwrap();
			proxy_tcp(client_stream, &target, &config).await
		});

		let mut conn = TcpStream::connect(proxy_addr).await.unwrap();
		conn.write_all(b"hello world").await.unwrap();

		let mut buf = Vec::new();
		conn.read_to_end(&mut buf).await.unwrap();

		let result = proxy_task.await.unwrap();
		assert!(result.is_ok(), "proxy_tcp failed: {result:?}");
		assert_eq!(buf, b"hello world");
	}

	#[tokio::test]
	async fn test_connect_timeout() {
		// 192.0.2.1 is TEST-NET-1 (RFC 5737), should be unreachable
		let target = ResolvedTarget { addr: "192.0.2.1:1".parse().unwrap() };

		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let proxy_addr = listener.local_addr().unwrap();

		let handle = tokio::spawn(async move {
			let _conn = TcpStream::connect(proxy_addr).await.unwrap();
			tokio::time::sleep(Duration::from_secs(5)).await;
		});

		let (client_stream, _) = listener.accept().await.unwrap();
		let config = ProxyConfig {
			connect_timeout: Duration::from_millis(100),
			idle_timeout: Duration::from_secs(10),
			watchdog_poll_interval: Duration::from_secs(1),
		};

		let start = tokio::time::Instant::now();
		let result = proxy_tcp(client_stream, &target, &config).await;
		let elapsed = start.elapsed();

		assert!(
			matches!(result, Err(ProxyError::ConnectTimeout { .. } | ProxyError::ConnectFailed { .. })),
			"expected ConnectTimeout or ConnectFailed, got {result:?}"
		);
		assert!(elapsed < Duration::from_secs(2), "took too long: {elapsed:?}");

		handle.abort();
	}

	#[tokio::test]
	async fn test_idle_timeout() {
		// Upstream that accepts but never sends/receives
		let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let upstream_addr = upstream_listener.local_addr().unwrap();

		let upstream_handle = tokio::spawn(async move {
			let (_stream, _) = upstream_listener.accept().await.unwrap();
			tokio::time::sleep(Duration::from_secs(30)).await;
		});

		let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let proxy_addr = proxy_listener.local_addr().unwrap();

		let client_handle = tokio::spawn(async move {
			let _conn = TcpStream::connect(proxy_addr).await.unwrap();
			tokio::time::sleep(Duration::from_secs(30)).await;
		});

		let (client_stream, _) = proxy_listener.accept().await.unwrap();
		let target = ResolvedTarget { addr: upstream_addr };
		let config = ProxyConfig {
			connect_timeout: Duration::from_secs(2),
			idle_timeout: Duration::from_millis(200),
			watchdog_poll_interval: Duration::from_millis(50),
		};

		let start = tokio::time::Instant::now();
		let result = proxy_tcp(client_stream, &target, &config).await;
		let elapsed = start.elapsed();

		assert!(
			matches!(result, Err(ProxyError::IdleTimeout { .. })),
			"expected IdleTimeout, got {result:?}"
		);
		assert!(
			elapsed >= Duration::from_millis(200) && elapsed < Duration::from_millis(500),
			"unexpected timing: {elapsed:?}"
		);

		upstream_handle.abort();
		client_handle.abort();
	}

	#[tokio::test]
	async fn connect_refused_returns_connect_failed() {
		// Bind then immediately drop to get a port that is definitely closed
		let tmp = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let closed_addr = tmp.local_addr().unwrap();
		drop(tmp);

		let target = ResolvedTarget { addr: closed_addr };

		let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let proxy_addr = proxy_listener.local_addr().unwrap();

		let client_handle = tokio::spawn(async move {
			let _conn = TcpStream::connect(proxy_addr).await.unwrap();
			tokio::time::sleep(Duration::from_secs(5)).await;
		});

		let (client_stream, _) = proxy_listener.accept().await.unwrap();
		let config = ProxyConfig {
			connect_timeout: Duration::from_secs(2),
			idle_timeout: Duration::from_secs(10),
			watchdog_poll_interval: Duration::from_secs(1),
		};

		let result = proxy_tcp(client_stream, &target, &config).await;
		assert!(
			matches!(result, Err(ProxyError::ConnectFailed { .. })),
			"expected ConnectFailed, got {result:?}"
		);

		client_handle.abort();
	}
}
