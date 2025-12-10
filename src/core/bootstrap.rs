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
use crate::modules::{
	nodes,
	plugins::loader as plugin_loader,
	ports,
	stack::protocol::carrier::{hotswap as resolver_hotswap, model as resolver_model},
};

pub async fn start() {
	dotenv().ok();
	setup_logging();
	print_motd();

	// CORRECTED STARTUP ORDER:
	// 1. Load nodes first.
	if let Some(initial_nodes) = nodes::hotswap::scan_nodes_config() {
		nodes::model::NODES_STATE.store(Arc::new(initial_nodes));
	}

	// 2. Load ports (L4 Listeners).
	let initial_ports = ports::hotswap::scan_ports_config();
	ports::model::CONFIG_STATE.store(Arc::new(initial_ports.clone()));

	// 3. Load Resolvers (L4+ Protocols).
	// This allows Vane to know how to handle upgraded connections (e.g., TLS, HTTP).
	let initial_resolvers = resolver_hotswap::scan_resolver_config();
	resolver_model::RESOLVER_REGISTRY.store(Arc::new(initial_resolvers));
	log(
		LogLevel::Info,
		&format!(
			"✓ Loaded {} resolver protocols.",
			resolver_model::RESOLVER_REGISTRY.load().len()
		),
	);

	// 4. Initialize background tasks (File Watchers & Health Checks).
	let config_change_receivers = requirements::initialize().await;

	// 5. Initialize External Plugins.
	plugin_loader::initialize();

	// Spawn hotswap listener for port changes.
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

	let unix_socket_listener = match socket::bind_unix_socket().await {
		Ok(listener) => Some(listener),
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to bind unix socket: {}", e),
			);
			None
		}
	};
	let requested_port = getenv::get_env("PORT", "3333".to_string())
		.parse::<u16>()
		.unwrap_or(0);
	let port = if portool::is_valid_port(requested_port) {
		requested_port
	} else {
		3333
	};
	let detect_public_network = getenv::to_lowercase(&getenv::get_env(
		"DETECT_PUBLIC_NETWORK",
		"true".to_string(),
	)) != "false";
	let listen_ipv6 =
		getenv::to_lowercase(&getenv::get_env("CONSOLE_LISTEN_IPV6", "false".to_string())) == "true";
	let addr: SocketAddr = if listen_ipv6 {
		([0; 8], port).into()
	} else {
		([0; 4], port).into()
	};

	let app = router::create_router();
	let shutdown_notifier = Arc::new(Notify::new());

	let tcp_notifier = shutdown_notifier.clone();
	let tcp_listener = TcpListener::bind(addr).await.unwrap();
	let tcp_server = serve(
		tcp_listener,
		app.clone().with_state(ports::model::CONFIG_STATE.clone()),
	)
	.with_graceful_shutdown(async move {
		tcp_notifier.notified().await;
	});

	let tcp_handle = tokio::spawn(async move {
		if let Err(e) = tcp_server.await {
			log(LogLevel::Error, &format!("✗ TCP console error: {}", e));
		}
	});

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

	let anynet_handle = task::spawn_blocking(move || {
		if detect_public_network {
			anynet!(port = port, public = true);
		} else {
			anynet!(port = port);
		}
	});

	tokio::spawn(async move {
		let timeout = sleep(Duration::from_millis(2100));
		tokio::select! {
			_ = anynet_handle => { log(LogLevel::Debug, "⚙ Anynet completed before timeout."); }
			_ = timeout => { log(LogLevel::Debug, "⚙ Anynet timeout reached."); }
		}

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
	});

	wait_for_shutdown_signal().await;
	log(LogLevel::Info, "➜ Signal received, shutdown now...");
	socket::cleanup_unix_socket();
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
