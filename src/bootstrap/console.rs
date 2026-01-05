/* src/bootstrap/console.rs */

use axum::serve;
use fancy_log::{LogLevel, log};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::api::middleware::auth;
use crate::api::router;
use crate::bootstrap::socket;
use crate::common::{config::env_loader, net::port_utils};
use crate::ingress::state;

pub struct ConsoleHandles {
	pub tcp_task: JoinHandle<()>,
	pub unix_task: Option<JoinHandle<()>>,
	pub shutdown_notifier: Arc<Notify>,
}

/// Initializes and starts the management console if ACCESS_TOKEN is configured.
pub async fn start() -> Option<ConsoleHandles> {
	match auth::validate_access_token() {
		Ok(None) => {
			log(
				LogLevel::Info,
				"⚙ Access token not set, management API disabled",
			);
			None
		}
		Ok(Some(_token)) => {
			log(LogLevel::Info, "✓ Access token configured");

			let unix_socket_listener = {
				#[cfg(feature = "console")]
				{
					match socket::bind_unix_socket().await {
						Ok(listener) => Some(listener),
						Err(e) => {
							log(
								LogLevel::Error,
								&format!("✗ Failed to bind unix socket: {}", e),
							);
							None
						}
					}
				}
				#[cfg(not(feature = "console"))]
				None
			};

			let requested_port = env_loader::get_env("PORT", "3333".to_string())
				.parse::<u16>()
				.unwrap_or(3333);
			let port = if port_utils::is_valid_port(requested_port) {
				requested_port
			} else {
				3333
			};

			let listen_ipv6 = env_loader::to_lowercase(&env_loader::get_env(
				"CONSOLE_LISTEN_IPV6",
				"false".to_string(),
			)) == "true";
			let addr: SocketAddr = if listen_ipv6 {
				([0; 8], port).into()
			} else {
				([0; 4], port).into()
			};

			let shutdown_notifier = Arc::new(Notify::new());

			#[cfg(feature = "console")]
			let app = router::create_router();

			#[cfg(feature = "console")]
			let tcp_handle = {
				let tcp_notifier = shutdown_notifier.clone();
				let tcp_listener = match TcpListener::bind(addr).await {
					Ok(l) => l,
					Err(e) => {
						log(
							LogLevel::Error,
							&format!("✗ Failed to bind TCP console: {}", e),
						);
						// Fallback if TCP fails but we want to return something?
						// Better to return None or exit. Bootstrap handles the exit.
						return None;
					}
				};
				log(LogLevel::Info, &format!("✓ TCP console bound to {}", addr));
				log(
					LogLevel::Info,
					&format!("✓ Listening on http://localhost:{}", port),
				);
				log(
					LogLevel::Info,
					&format!("✓ Listening on http://127.0.0.1:{}", port),
				);

				let tcp_server = serve(
					tcp_listener,
					app.clone().with_state(state::CONFIG_STATE.clone()),
				)
				.with_graceful_shutdown(async move {
					tcp_notifier.notified().await;
				});

				tokio::spawn(async move {
					if let Err(e) = tcp_server.await {
						log(LogLevel::Error, &format!("✗ TCP console error: {}", e));
					}
				})
			};
			#[cfg(not(feature = "console"))]
			let tcp_handle = tokio::spawn(async {});

			#[cfg(feature = "console")]
			let unix_handle = if let Some(listener) = unix_socket_listener {
				let unix_notifier = shutdown_notifier.clone();
				let unix_server = serve(listener, app.with_state(state::CONFIG_STATE.clone()))
					.with_graceful_shutdown(async move {
						unix_notifier.notified().await;
					});
				Some(tokio::spawn(async move {
					if let Err(e) = unix_server.await {
						log(
							LogLevel::Error,
							&format!("✗ Unix socket console error: {}", e),
						);
					}
				}))
			} else {
				None
			};
			#[cfg(not(feature = "console"))]
			let unix_handle = None;

			Some(ConsoleHandles {
				tcp_task: tcp_handle,
				unix_task: unix_handle,
				shutdown_notifier,
			})
		}
		Err(err_msg) => {
			log(LogLevel::Error, &format!("✗ {}", err_msg));
			std::process::exit(1);
		}
	}
}

/// Performs cleanup and graceful shutdown of the console servers.
pub async fn stop(handles: ConsoleHandles) {
	socket::cleanup_unix_socket().await;
	handles.shutdown_notifier.notify_waiters();

	if let Some(handle) = handles.unix_task {
		let _ = tokio::join!(handles.tcp_task, handle);
	} else {
		let _ = handles.tcp_task.await;
	}
}
