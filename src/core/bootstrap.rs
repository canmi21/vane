/* src/core/bootstrap.rs */

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

use crate::common::{getenv, portool, requirements};
use crate::core::{router, socket};
use crate::middleware::auth;
use crate::modules::{
	certs, nodes,
	plugins::core::loader as plugin_loader,
	ports,
	stack::{
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
			log(
				LogLevel::Info,
				"⚙ ACCESS_TOKEN not set, management API disabled",
			);
			None
		}
		Ok(Some(_token)) => {
			log(
				LogLevel::Info,
				"✓ ACCESS_TOKEN configured (management console enabled)",
			);

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

			let requested_port = getenv::get_env("PORT", "3333".to_string())
				.parse::<u16>()
				.unwrap_or(3333);
			let port = if portool::is_valid_port(requested_port) {
				requested_port
			} else {
				3333
			};

			let listen_ipv6 =
				getenv::to_lowercase(&getenv::get_env("CONSOLE_LISTEN_IPV6", "false".to_string()))
					== "true";
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
						return;
					}
				};
				log(LogLevel::Info, &format!("✓ TCP console bound to {}", addr));
				// Python tests look for these specific logs:
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
			#[cfg(not(feature = "console"))]
			let tcp_handle = tokio::spawn(async {});

			#[cfg(feature = "console")]
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
			#[cfg(not(feature = "console"))]
			let unix_handle = None;

			Some((tcp_handle, unix_handle, shutdown_notifier))
		}
		Err(err_msg) => {
			log(LogLevel::Error, &format!("✗ {}", err_msg));
			std::process::exit(1);
		}
	};

	wait_for_shutdown_signal().await;
	log(LogLevel::Info, "➜ Signal received, shutdown now...");

	if let Some((tcp_handle, unix_handle, shutdown_notifier)) = console_handles {
		socket::cleanup_unix_socket().await;
		shutdown_notifier.notify_waiters();

		if let Some(handle) = unix_handle {
			let _ = tokio::join!(tcp_handle, handle);
		} else {
			let _ = tcp_handle.await;
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
