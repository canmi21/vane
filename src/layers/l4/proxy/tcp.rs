/* src/layers/l4/proxy/tcp.rs */

use super::IdleWatchdog;
use crate::common::config::getenv;
use crate::layers::l4::{health, model::ResolvedTarget};
use anyhow::{Context, Result};
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::{
	net::TcpStream,
	time::{Duration, timeout},
};

// Constants
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn proxy_tcp_stream(client_stream: TcpStream, target: ResolvedTarget) -> Result<()> {
	log(
		LogLevel::Debug,
		&format!(
			"➜ TCP Proxy connecting to upstream: {}:{}",
			target.ip, target.port
		),
	);

	let connect_result = timeout(
		CONNECT_TIMEOUT,
		TcpStream::connect(format!("{}:{}", target.ip, target.port)),
	)
	.await;

	let upstream_stream = match connect_result {
		Ok(Ok(stream)) => stream,
		Ok(Err(e)) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to connect to upstream target {}:{}: {}",
					target.ip, target.port, e
				),
			);
			health::mark_tcp_target_unhealthy(&target);
			return Err(anyhow::Error::new(e).context("Failed to connect to upstream"));
		}
		Err(_) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Timeout connecting to upstream target {}:{}",
					target.ip, target.port
				),
			);
			health::mark_tcp_target_unhealthy(&target);
			return Err(anyhow::anyhow!("Connection timed out"));
		}
	};

	let _ = client_stream.set_nodelay(true);
	let _ = upstream_stream.set_nodelay(true);

	// Implement Idle Timeout
	let last_activity = Arc::new(AtomicU64::new(
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
	));

	let timeout_secs = getenv::get_env("STREAM_IDLE_TIMEOUT_SECS", "10".to_string())
		.parse::<u64>()
		.unwrap_or(10);

	let mut client_wrapped = IdleWatchdog::new(client_stream, last_activity.clone());
	let mut upstream_wrapped = IdleWatchdog::new(upstream_stream, last_activity.clone());

	let (mut client_read, mut client_write) = tokio::io::split(&mut client_wrapped);
	let (mut upstream_read, mut upstream_write) = tokio::io::split(&mut upstream_wrapped);

	let client_to_server = tokio::io::copy(&mut client_read, &mut upstream_write);
	let server_to_client = tokio::io::copy(&mut upstream_read, &mut client_write);

	let watchdog = async {
		loop {
			tokio::time::sleep(Duration::from_secs(1)).await;
			let last = last_activity.load(Ordering::Relaxed);
			let now = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs();
			if now - last >= timeout_secs {
				break;
			}
		}
	};

	tokio::select! {
		res = client_to_server => res.map(|_| ()).context("Client->Server copy failed"),
		res = server_to_client => res.map(|_| ()).context("Server->Client copy failed"),
		_ = watchdog => {
			log(LogLevel::Warn, "⚠ Security: Stream idle timeout triggered (TCP).");
			Err(anyhow::anyhow!("Stream idle timeout"))
		}
	}
}
