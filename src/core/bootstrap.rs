/* src/core/bootstrap.rs */

use anynet::anynet;
use axum::serve;
use dotenvy::dotenv;
use fancy_log::{LogLevel, log, set_log_level};
use lazy_motd::lazy_motd;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::Notify;
use tokio::task;
use tokio::time::{Duration, sleep};

use crate::common::{getenv, portool, requirements};
use crate::core::{router, socket};
use crate::middleware::auth;
use crate::modules::{
	certs, nodes,
	plugins::loader as plugin_loader,
	ports,
	stack::protocol::{
		application::{hotswap as app_hotswap, model as app_model},
		carrier::{hotswap as resolver_hotswap, model as resolver_model},
	},
};

pub async fn start() {
	#[cfg(feature = "aws-lc-rs")]
	{
		use rustls::crypto::aws_lc_rs;
		aws_lc_rs::default_provider()
			.install_default()
			.expect("failed to install aws-lc-rs crypto provider");
	}

	#[cfg(feature = "ring")]
	{
		use rustls::crypto::ring;
		ring::default_provider()
			.install_default()
			.expect("failed to install ring crypto provider");
	}

	dotenv().ok();
	setup_logging();
	print_motd();

	// CORRECTED STARTUP ORDER:

	// 1. Ensure Config Files Exist
	requirements::ensure_config_files_exist().await;

	// 2. Load nodes first.
	if let Some(initial_nodes) = nodes::hotswap::scan_nodes_config().await {
		nodes::model::NODES_STATE.store(Arc::new(initial_nodes));
	}

	// 3. Load Certificates (Keep-Last-Good).
	certs::loader::initialize().await;

	// 4. Load ports (L4 Listeners).
	let initial_ports = ports::hotswap::scan_ports_config(&[]).await;
	ports::model::CONFIG_STATE.store(Arc::new(initial_ports.clone()));

	// 5. Load Resolvers (L4+ Protocols).
	let initial_resolvers =
		resolver_hotswap::scan_resolver_config(&resolver_model::RESOLVER_REGISTRY.load()).await;
	resolver_model::RESOLVER_REGISTRY.store(Arc::new(initial_resolvers));
	log(
		LogLevel::Info,
		&format!(
			"✓ Loaded {} resolver protocols.",
			resolver_model::RESOLVER_REGISTRY.load().len()
		),
	);

	// 6. Load Applications (L7 Protocols).
	let initial_apps =
		app_hotswap::scan_application_config(&app_model::APPLICATION_REGISTRY.load()).await;
	app_model::APPLICATION_REGISTRY.store(Arc::new(initial_apps));
	log(
		LogLevel::Info,
		&format!(
			"✓ Loaded {} application protocols.",
			app_model::APPLICATION_REGISTRY.load().len()
		),
	);

	// 7. Initialize background tasks (Health Checks & Session Cleanup).
	requirements::start_background_tasks().await;

	// 8. Initialize External Plugins.
	plugin_loader::initialize().await;

	// 9. Start Listeners IMMEDIATELY
	log(
		LogLevel::Info,
		"⚙ Initializing listeners from existing config...",
	);
	let ip_version_str =
		if getenv::get_env("LISTEN_IPV6", "false".to_string()).to_lowercase() == "true" {
			"IPv4 + IPv6"
		} else {
			"IPv4"
		};
	for status in &initial_ports {
		if status.tcp_config.is_some() {
			log(
				LogLevel::Info,
				&format!("↑ {} PORT {} TCP UP", ip_version_str, status.port),
			);
			ports::listener::start_listener(status.port, ports::model::Protocol::Tcp);
		}
		if status.udp_config.is_some() {
			log(
				LogLevel::Info,
				&format!("↑ {} PORT {} UDP UP", ip_version_str, status.port),
			);
			ports::listener::start_listener(status.port, ports::model::Protocol::Udp);
		}
	}

	// 10. Start Config Watchers
	let config_change_receivers = requirements::start_config_watchers_only();

	// 11. Spawn Hotswap Listeners
	tokio::spawn(ports::hotswap::listen_for_updates(
		config_change_receivers.ports,
	));

	// Spawn hotswap listener for node changes.
	tokio::spawn(nodes::hotswap::listen_for_updates(
		config_change_receivers.nodes,
	));

	// Spawn hotswap listener for resolver changes.
	tokio::spawn(resolver_hotswap::listen_for_updates(
		config_change_receivers.resolvers,
	));

	// Spawn hotswap listener for certificate changes.
	tokio::spawn(certs::loader::listen_for_updates(
		config_change_receivers.certs,
	));

	// Spawn hotswap listener for application changes.
	tokio::spawn(app_hotswap::listen_for_updates(
		config_change_receivers.applications,
	));

	// Validate ACCESS_TOKEN and conditionally start management console
	let console_handles = match auth::validate_access_token() {
		Ok(None) => {
			// ACCESS_TOKEN not set - management API disabled
			log(
				LogLevel::Info,
				"⚙ ACCESS_TOKEN not set, management API disabled",
			);
			log(
				LogLevel::Info,
				"  To enable console: export ACCESS_TOKEN=$(openssl rand -hex 32)",
			);
			// Skip console startup, only start business listeners
			None
		}
		Ok(Some(_token)) => {
			// ACCESS_TOKEN valid - start management console
			log(
				LogLevel::Info,
				"✓ ACCESS_TOKEN configured (management console enabled)",
			);

			let unix_socket_listener = {
				#[cfg(feature = "unix-console")]
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
				#[cfg(not(feature = "unix-console"))]
				None
			};

			let requested_port = getenv::get_env("PORT", "3333".to_string())
				.parse::<u16>()
				.unwrap_or(0);
			let port = if portool::is_valid_port(requested_port) {
				requested_port
			} else {
				3333
			};
			let _detect_public_network = getenv::to_lowercase(&getenv::get_env(
				"DETECT_PUBLIC_NETWORK",
				"true".to_string(),
			)) != "false";
			let listen_ipv6 =
				getenv::to_lowercase(&getenv::get_env("CONSOLE_LISTEN_IPV6", "false".to_string()))
					== "true";
			let addr: SocketAddr = if listen_ipv6 {
				([0; 8], port).into()
			} else {
				([0; 4], port).into()
			};

			let shutdown_notifier = Arc::new(Notify::new());

			#[cfg(any(feature = "http-console", feature = "unix-console"))]
			let app = router::create_router();

			#[cfg(feature = "http-console")]
			let tcp_handle = {
				let tcp_notifier = shutdown_notifier.clone();
				let tcp_listener = match TcpListener::bind(addr).await {
					Ok(l) => l,
					Err(e) => {
						log(
							LogLevel::Error,
							&format!("✗ Failed to bind TCP console to {}: {}", addr, e),
						);
						return;
					}
				};
				log(LogLevel::Info, &format!("✓ TCP console bound to {}", addr));

				let tcp_server = serve(
					tcp_listener,
					app.clone().with_state(ports::model::CONFIG_STATE.clone()),
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
			#[cfg(not(feature = "http-console"))]
			let tcp_handle = tokio::spawn(async {});

			#[cfg(feature = "unix-console")]
			let unix_handle = if let Some(listener) = unix_socket_listener {
				let unix_notifier = shutdown_notifier.clone();
				let unix_server = serve(listener, app.with_state(ports::model::CONFIG_STATE.clone()))
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
			#[cfg(not(feature = "unix-console"))]
			let unix_handle = None;

			// Return console handles and shutdown notifier
			Some((tcp_handle, unix_handle, shutdown_notifier))
		}
		Err(err_msg) => {
			// ACCESS_TOKEN length invalid - refuse to start
			log(LogLevel::Error, &format!("✗ {}", err_msg));
			log(
				LogLevel::Error,
				"⚠ ACCESS_TOKEN length invalid (requires 16-128 chars), management API disabled",
			);
			log(
				LogLevel::Error,
				"✗ Vane refuses to start with invalid ACCESS_TOKEN",
			);
			std::process::exit(1);
		}
	};

	// Business port initialization (runs regardless of console status)
	// Determine port for anynet based on console status
	let anynet_port = if let Some((_, _, _)) = &console_handles {
		// Console is running, use its port for anynet
		getenv::get_env("PORT", "3333".to_string())
			.parse::<u16>()
			.unwrap_or(3333)
	} else {
		// Console not running, skip anynet or use a default
		0 // Will skip anynet call
	};

	if anynet_port > 0 {
		let detect_public = getenv::to_lowercase(&getenv::get_env(
			"DETECT_PUBLIC_NETWORK",
			"true".to_string(),
		)) != "false";

		let anynet_handle = task::spawn_blocking(move || {
			if detect_public {
				anynet!(port = anynet_port, public = true);
			} else {
				anynet!(port = anynet_port);
			}
		});

		tokio::spawn(async move {
			let timeout = sleep(Duration::from_millis(2100));
			tokio::select! {
				_ = anynet_handle => { log(LogLevel::Debug, "⚙ Anynet completed before timeout."); }
				_ = timeout => { log(LogLevel::Debug, "⚙ Anynet timeout reached."); }
			}
		});
	}

	// Wait for shutdown signal
	wait_for_shutdown_signal().await;
	log(LogLevel::Info, "➜ Signal received, shutdown now...");

	// Conditionally shutdown console if it was started
	if let Some((tcp_handle, unix_handle, shutdown_notifier)) = console_handles {
		socket::cleanup_unix_socket().await;
		shutdown_notifier.notify_waiters();

		if let Some(handle) = unix_handle {
			let (tcp_res, unix_res) = tokio::join!(tcp_handle, handle);
			if let Err(e) = tcp_res {
				log(
					LogLevel::Error,
					&format!("✗ TCP console task failed: {}", e),
				);
			}
			if let Err(e) = unix_res {
				log(
					LogLevel::Error,
					&format!("✗ Unix console task failed: {}", e),
				);
			}
		} else {
			if let Err(e) = tcp_handle.await {
				log(
					LogLevel::Error,
					&format!("✗ TCP console task failed: {}", e),
				);
			}
		}
	}

	log(LogLevel::Info, "✓ Server has been shut down gracefully.");
}

fn setup_logging() {
	let level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
	let log_level = match level.to_lowercase().as_str() {
		"debug" => LogLevel::Debug,
		"warn" => LogLevel::Warn,
		"error" => LogLevel::Error,
		_ => LogLevel::Info,
	};
	set_log_level(log_level);
}

fn print_motd() {
	lazy_motd!(
		environment = "None",
		build = "Nightly",
		copyright = &[
			"Copyright (c) 2025 Canmi and contributors",
			"Github OSS Released under the MIT License."
		]
	);
}

async fn wait_for_shutdown_signal() {
	let ctrl_c = async {
		signal::ctrl_c()
			.await
			.expect("failed to install Ctrl+C handler");
	};
	#[cfg(unix)]
	let terminate = async {
		signal::unix::signal(signal::unix::SignalKind::terminate())
			.expect("failed to install signal handler")
			.recv()
			.await;
	};
	#[cfg(not(unix))]
	let terminate = std::future::pending::<()>();
	tokio::select! { _ = ctrl_c => {}, _ = terminate => {}, }
}
