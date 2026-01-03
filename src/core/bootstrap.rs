/* src/core/bootstrap.rs */

use dotenvy::dotenv;
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio::signal;

use crate::common::{getenv, lifecycle, watcher};
use crate::core::{console, logging, monitor};
use crate::modules::{
	certs, nodes,
	plugins::core::loader as plugin_loader,
	ports,
	stack::{
		application::{hotswap as app_hotswap, model as app_model},
		carrier::{hotswap as resolver_hotswap, model as resolver_model},
	},
};

/// Entry point for the Vane bootstrap sequence.
pub async fn start() {
	// Initialize Crypto Backends
	setup_crypto();

	// Load Environment and Logging
	dotenv().ok();
	logging::setup();
	logging::print_motd();

	// 1. Infrastructure Readiness
	lifecycle::ensure_config_files_exist().await;

	// 2. Load Service Discovery (Nodes)
	if let Some(initial_nodes) = nodes::hotswap::scan_nodes_config().await {
		nodes::model::NODES_STATE.store(Arc::new(initial_nodes));
	}

	// 3. Load Certificates (TLS)
	certs::loader::initialize().await;

	// 4. Load Port Configurations (L4 Listeners)
	let initial_ports = ports::hotswap::scan_ports_config(&[]).await;
	ports::model::CONFIG_STATE.store(Arc::new(initial_ports.clone()));

	// 5. Load L4+ Resolvers
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

	// 6. Load Applications (L7 Protocols)
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

	// 7. Start Background Maintenance Tasks
	lifecycle::start_background_tasks().await;

	// 8. Initialize Plugin System
	plugin_loader::initialize().await;

	// 8.5 Initialize Adaptive Resource Management
	monitor::start_l7_memory_monitor().await;

	// 9. Activate Listeners
	start_initial_listeners(&initial_ports).await;

	// 10. Start Configuration Hotswap System
	let receivers = watcher::start_config_watchers_only();
	spawn_hotswap_tasks(receivers).await;

	// 11. Start Management Plane (Console)
	let console_handles = console::start().await;

	// 12. Run until Shutdown Signal
	wait_for_shutdown_signal().await;
	log(LogLevel::Info, "➜ Signal received, shutdown now...");

	// 13. Graceful Shutdown Cleanup
	if let Some(handles) = console_handles {
		console::stop(handles).await;
	}

	log(LogLevel::Info, "✓ Server has been shut down gracefully.");
}

fn setup_crypto() {
	#[cfg(feature = "aws-lc-rs")]
	{
		use rustls::crypto::aws_lc_rs;
		let _ = aws_lc_rs::default_provider().install_default();
	}

	#[cfg(feature = "ring")]
	{
		use rustls::crypto::ring;
		let _ = ring::default_provider().install_default();
	}
}

async fn start_initial_listeners(ports: &[ports::model::PortStatus]) {
	log(
		LogLevel::Info,
		"⚙ Initializing listeners from existing config...",
	);
	let ip_version = if getenv::get_env("LISTEN_IPV6", "false".to_string()).to_lowercase() == "true" {
		"IPv4 + IPv6"
	} else {
		"IPv4"
	};

	for status in ports {
		if status.tcp_config.is_some() {
			log(
				LogLevel::Info,
				&format!("↑ {} PORT {} TCP UP", ip_version, status.port),
			);
			ports::listener::start_listener(status.port, ports::model::Protocol::Tcp);
		}
		if status.udp_config.is_some() {
			log(
				LogLevel::Info,
				&format!("↑ {} PORT {} UDP UP", ip_version, status.port),
			);
			ports::listener::start_listener(status.port, ports::model::Protocol::Udp);
		}
	}
}

async fn spawn_hotswap_tasks(receivers: watcher::ConfigChangeReceivers) {
	tokio::spawn(ports::hotswap::listen_for_updates(receivers.ports));
	tokio::spawn(nodes::hotswap::listen_for_updates(receivers.nodes));
	tokio::spawn(resolver_hotswap::listen_for_updates(receivers.resolvers));
	tokio::spawn(certs::loader::listen_for_updates(receivers.certs));
	tokio::spawn(app_hotswap::listen_for_updates(receivers.applications));
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
