/* src/core/bootstrap.rs */

use anynet::anynet;
use axum::serve;
use dotenvy::dotenv;
use fancy_log::{LogLevel, log, set_log_level};
use lazy_motd::lazy_motd;
use std::env;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::task;

use crate::common::{getenv, portool};
use crate::core::router;

pub async fn start() {
	dotenv().ok();
	setup_logging();
	print_motd();

	// Get port from environment, parse, and validate.
	let requested_port = getenv::get_env("PORT", "3333".to_string())
		.parse::<u16>()
		.unwrap_or(0); // Default to 0 on parse error for validation.

	let port = if portool::is_valid_port(requested_port) {
		requested_port
	} else {
		3333 // Fallback to default port if invalid.
	};

	let detect_public_network = getenv::to_lowercase(&getenv::get_env(
		"DETECT_PUBLIC_NETWORK",
		"true".to_string(),
	)) != "false";

	// Determine whether to listen on IPv6 or IPv4.
	let listen_ipv6 =
		getenv::to_lowercase(&getenv::get_env("CONSOLE_LISTEN_IPV6", "false".to_string())) == "true";

	let addr: SocketAddr = if listen_ipv6 {
		([0, 0, 0, 0, 0, 0, 0, 0], port).into() // Listen on :: (IPv6)
	} else {
		([0, 0, 0, 0], port).into() // Listen on 0.0.0.0 (IPv4)
	};

	let app = router::create_router();

	let port_clone = port;
	if let Err(e) = task::spawn_blocking(move || {
		if detect_public_network {
			anynet!(port = port_clone, public = true);
		} else {
			anynet!(port = port_clone);
		}
	})
	.await
	{
		log(
			LogLevel::Error,
			&format!("anynet panicked in a blocking task: {}", e),
		);
	}

	let listener = TcpListener::bind(addr).await.unwrap();

	let console_server =
		serve(listener, app.into_make_service()).with_graceful_shutdown(shutdown_signal());

	let console_handle = tokio::spawn(async move {
		if let Err(e) = console_server.await {
			log(
				LogLevel::Error,
				&format!("Management console server error: {}", e),
			);
		}
	});

	if let Err(e) = console_handle.await {
		log(
			LogLevel::Error,
			&format!("Management console task failed: {}", e),
		);
	}

	log(LogLevel::Info, "Server has been shut down gracefully.");
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

async fn shutdown_signal() {
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

	log(LogLevel::Info, "Signal received, shutdown now...");
}
