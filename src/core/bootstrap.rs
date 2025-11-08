/* src/core/bootstrap.rs */

use anynet::anynet;
use arc_swap::ArcSwap;
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

use crate::common::{getenv, portool, requirements};
use crate::core::{router, socket};
use crate::modules::ports::{hotswap, listener, model::PortState};

pub async fn start() {
	dotenv().ok();
	setup_logging();
	print_motd();

	let config_change_receiver = requirements::initialize().await;

	// --- Initialize Port State and Start Listeners ---
	let initial_ports = hotswap::scan_ports_config();
	log(
		LogLevel::Info,
		"⚙ Initializing listeners from existing config...",
	);
	for status in &initial_ports {
		if status.active {
			for protocol in &status.protocols {
				let proto_str = format!("{:?}", protocol).to_uppercase();
				log(
					LogLevel::Info,
					&format!("↑ PORT {} {} UP", status.port, proto_str),
				);
				listener::start_listener(status.port, protocol.clone());
			}
		}
	}

	let port_state: PortState = Arc::new(ArcSwap::new(Arc::new(initial_ports)));
	tokio::spawn(hotswap::listen_for_updates(
		port_state.clone(),
		config_change_receiver,
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

	if let Err(e) = task::spawn_blocking(move || {
		if detect_public_network {
			anynet!(port = port, public = true);
		} else {
			anynet!(port = port);
		}
	})
	.await
	{
		log(
			LogLevel::Error,
			&format!("✗ Anynet panicked in a blocking task: {}", e),
		);
	}

	let shutdown_notifier = Arc::new(Notify::new());

	let tcp_notifier = shutdown_notifier.clone();
	let tcp_listener = TcpListener::bind(addr).await.unwrap();
	let tcp_server = serve(tcp_listener, app.clone().with_state(port_state.clone()))
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
		let unix_server =
			serve(listener, app.with_state(port_state)).with_graceful_shutdown(async move {
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

	tokio::select! {
		_ = ctrl_c => {},
		_ = terminate => {},
	}
}
