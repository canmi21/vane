/* engine/src/daemon/bootstrap.rs */

use crate::config::{template, uuid};
use crate::daemon::{config, console, router};
use crate::modules::domain::entrance as domain_helper;
use crate::modules::layout::manager as layout_manager;
use crate::modules::origins::task as origin_monitor_task;
use crate::modules::plugins::manager as plugins_manager;
use crate::proxy::domain;
use crate::proxy::router::generate::generate_router_tree;
use crate::proxy::router::watch::{initial_load_all_routers, start_router_watcher};
use crate::servers;
use anynet::anynet;
use axum::serve;
use dotenvy::dotenv;
use fancy_log::{LogLevel, log, set_log_level};
use lazy_motd::lazy_motd;
use std::env;
use std::net::SocketAddr;
use std::process;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::task;

/// Scans all domains and generates their router trees on startup.
async fn generate_all_router_trees() {
	log(
		LogLevel::Info,
		"Generating router trees for all domains on startup...",
	);
	let domains = domain_helper::list_domains_internal().await;
	for domain in domains {
		generate_router_tree(&domain).await;
	}
	log(LogLevel::Info, "Router tree generation complete.");
}

pub async fn start() {
	dotenv().ok();
	setup_logging();
	print_motd();

	config::initialize_config_directory();
	template::initialize_templates();
	console::initialize_console_config().await;

	if let Err(e) = uuid::initialize_instance_config().await {
		log(
			LogLevel::Error,
			&format!("Failed to initialize instance configuration: {}", e),
		);
		log(
			LogLevel::Error,
			"Please check file permissions for the config directory and restart.",
		);
		process::exit(1);
	}

	origin_monitor_task::initialize_monitor_config().await;
	origin_monitor_task::start_monitoring_task();

	plugins_manager::initialize_plugins().await;

	layout_manager::initialize_all_layout_configs().await;
	generate_all_router_trees().await;

	initial_load_all_routers().await;
	start_router_watcher();

	domain::initial_load_domains().await;
	domain::start_domain_watchdog();

	let port = env::var("PORT")
		.ok()
		.and_then(|s| s.parse::<u16>().ok())
		.unwrap_or(3333);

	let detect_public_network = env::var("DETECT_PUBLIC_NETWORK")
		.unwrap_or_else(|_| "true".to_string())
		.to_lowercase()
		!= "false";

	let app = router::create_router();
	let addr = SocketAddr::from(([0, 0, 0, 0], port));

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

	log(
		LogLevel::Info,
		&format!("Management console listening on {}", addr),
	);

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

	servers::start_proxy_servers().await;

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
			"OSS Released under the AGPL-3.0 License."
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
