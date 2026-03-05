/* src/core/src/bootstrap/startup.rs */

use fancy_log::{LogLevel, log};
use live::signal::Config as WatcherConfig;
use sigterm;

use crate::bootstrap::{console, logging, monitor, plugins};
use vane_engine::config::{self, ConfigManager};
use vane_extra::core::loader as plugin_loader;
use vane_primitives::certs;
use vane_primitives::common::sys::lifecycle;
use vane_transport::ingress::{hotswap, listener, state};

/// Entry point for the Vane bootstrap sequence.
#[allow(clippy::too_many_lines)]
pub async fn start() {
	// Initialize Crypto Backends
	setup_crypto();

	// Load Environment and Logging
	envflag::init().ok();
	logging::setup();
	logging::print_motd();

	// 1. Infrastructure Readiness
	lifecycle::ensure_config_files_exist().await;

	// 2. Register Internal Plugins (must happen before config validation)
	plugins::register_builtin_plugins();

	// 3. Initialize Config Manager
	let config_dir_path = vane_primitives::common::config::file_loader::get_config_dir();
	let config_dir_str = config_dir_path.to_str().expect("Config dir path is not valid UTF-8");

	let mut config = match ConfigManager::init(config_dir_str).await {
		Ok(c) => c,
		Err(e) => {
			log(LogLevel::Error, &format!("Failed to initialize config: {e}"));
			return;
		}
	};

	// 3. Load Configurations
	match config.listeners.tcp.load().await {
		Ok(result) => {
			for (key, error) in &result.failed {
				if error.to_lowercase().contains("validation") {
					log(LogLevel::Error, &format!("✗ Validation failed for TCP listener [{key}]: {error}"));
				} else {
					log(
						LogLevel::Error,
						&format!("✗ Failed to parse config file for TCP listener [{key}]: {error}"),
					);
				}
			}
		}
		Err(e) => log(LogLevel::Error, &format!("Failed to load TCP listeners: {e}")),
	}

	match config.listeners.udp.load().await {
		Ok(result) => {
			for (key, error) in &result.failed {
				if error.to_lowercase().contains("validation") {
					log(LogLevel::Error, &format!("✗ Validation failed for UDP listener [{key}]: {error}"));
				} else {
					log(
						LogLevel::Error,
						&format!("✗ Failed to parse config file for UDP listener [{key}]: {error}"),
					);
				}
			}
		}
		Err(e) => log(LogLevel::Error, &format!("Failed to load UDP listeners: {e}")),
	}

	match config.resolvers.load().await {
		Ok(result) => {
			for (key, error) in &result.failed {
				if error.to_lowercase().contains("validation") {
					log(LogLevel::Error, &format!("✗ Validation failed for resolver [{key}]: {error}"));
				} else {
					log(
						LogLevel::Error,
						&format!("✗ Failed to parse config file for resolver [{key}]: {error}"),
					);
				}
			}
		}
		Err(e) => log(LogLevel::Error, &format!("Failed to load resolvers: {e}")),
	}

	match config.applications.load().await {
		Ok(result) => {
			for (key, error) in &result.failed {
				if error.to_lowercase().contains("validation") {
					log(LogLevel::Error, &format!("✗ Validation failed for application [{key}]: {error}"));
				} else {
					log(
						LogLevel::Error,
						&format!("✗ Failed to parse config file for application [{key}]: {error}"),
					);
				}
			}
		}
		Err(e) => log(LogLevel::Error, &format!("Failed to load applications: {e}")),
	}
	// Nodes - suppress error if file not found (default behavior)
	match config.nodes.load().await {
		Ok(_) => log(LogLevel::Debug, "⚙ Loaded nodes configuration."),
		Err(live::controller::LiveError::Load(live::loader::FmtError::NotFound)) => {
			log(LogLevel::Debug, "⚙ Nodes configuration file not found. Using default.");
		}
		Err(e) => log(LogLevel::Error, &format!("Failed to load nodes: {e}")),
	}

	// 3.5 LazyCert (Hybrid)
	if let Some(lc) = &config.lazycert {
		// Try load, ignore not found
		let _ = lc.load().await;
	}

	// 4. Start Configuration Hotswap System (Before moving config)
	let watch_config = WatcherConfig::default();

	// Start watchers for Live components
	let _ = config.listeners.start_watching(watch_config.clone()).await;
	let _ = config.resolvers.start_watching(watch_config.clone()).await;
	let _ = config.applications.start_watching(watch_config.clone()).await;
	let _ = config.nodes.start_watching(watch_config.clone()).await;
	if let Some(lc) = &mut config.lazycert {
		let _ = lc.start_watching(watch_config.clone()).await;
	}

	// 5. Set Global Config
	if config::CONFIG.set(config).is_err() {
		panic!("Config already initialized");
	}
	let config = config::get();

	// 6. Load Certificates (TLS) - Legacy/Custom for now
	certs::loader::initialize().await;

	// 7. Initialize LazyCert integration (after config set)
	crate::lazycert::initialize().await;

	// 8. Start Background Maintenance Tasks
	start_background_tasks().await;

	// 9. Load External Plugins
	plugin_loader::initialize().await;

	// 10. Initialize Adaptive Resource Management
	monitor::start_l7_memory_monitor().await;

	// 11. Activate Listeners
	start_initial_listeners(config).await;

	// 12. Spawn listener event loop
	tokio::spawn(hotswap::start_listener_event_loop(config));

	// 13. Custom watcher for Certs
	start_certs_watcher(config_dir_path.join("certs")).await;

	// 14. Start Management Plane (Console)
	let console_handles = console::start().await;

	// 15. Run until Shutdown Signal
	sigterm::wait().await;
	log(LogLevel::Info, "➜ Signal received, shutdown now...");

	// 16. Graceful Shutdown Cleanup
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

async fn start_initial_listeners(config: &ConfigManager) {
	log(LogLevel::Info, "⚙ Initializing listeners from existing config...");

	// TCP
	let tcp_map = config.listeners.tcp.snapshot().await;
	for (port_str, _) in tcp_map {
		if let Ok(port) = port_str.parse::<u16>() {
			log(LogLevel::Info, &format!("↑ PORT {port} TCP UP"));
			listener::start_listener(port, state::Protocol::Tcp);
		}
	}

	// UDP
	let udp_map = config.listeners.udp.snapshot().await;
	for (port_str, _) in udp_map {
		if let Ok(port) = port_str.parse::<u16>() {
			log(LogLevel::Info, &format!("↑ PORT {port} UDP UP"));
			listener::start_listener(port, state::Protocol::Udp);
		}
	}
}

async fn start_certs_watcher(cert_dir: std::path::PathBuf) {
	use live::signal::{Config as WatcherConfig, Target, Watcher};

	let target = Target::Filtered {
		path: cert_dir,
		include: vec!["*.pem".to_owned(), "*.crt".to_owned(), "*.key".to_owned()],
		exclude: vec!["*.bak".to_owned()],
	};

	match Watcher::new(target, WatcherConfig::default()) {
		Ok(watcher) => {
			tokio::spawn(async move {
				let _watcher = watcher; // Keep alive
				let mut rx = _watcher.subscribe();
				while rx.recv().await.is_ok() {
					vane_primitives::certs::loader::scan_and_load_certs().await;
				}
			});
		}
		Err(e) => {
			log(LogLevel::Error, &format!("✗ Failed to start certs watcher: {e}"));
		}
	}
}

/// Spawns essential background maintenance tasks (moved from lifecycle).
async fn start_background_tasks() {
	use vane_engine::shared::{health, session};
	use vane_transport::l4p::quic::session as quic_session;

	health::initial_health_check().await;
	health::start_periodic_health_checkers();
	session::start_session_cleanup_task();
	quic_session::start_cleanup_task();
}
