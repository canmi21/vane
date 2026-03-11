use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use vane_engine::config::ConfigTable;
use vane_engine::engine::Engine;
use vane_engine::flow::default_plugin_registry;
use vane_panel::{VaneState, start_panel_server};
use vane_transport::tls::CertStore;

const DEFAULT_PANEL_BIND_ADDR: &str = "127.0.0.1:3333";

#[tokio::main]
async fn main() -> Result<()> {
	init_tracing();

	let started_at = SystemTime::now();
	let panel_bind_addr = panel_bind_addr()?;
	let initial_config = load_initial_config()?;

	let mut engine = Engine::new(initial_config, default_plugin_registry(), CertStore::new())
		.context("failed to build engine")?;
	engine.start().await.context("failed to start engine")?;

	let engine = Arc::new(engine);
	let state = Arc::new(VaneState::new(Arc::clone(&engine), started_at));
	let panel_task = tokio::spawn(start_panel_server(Arc::clone(&state), panel_bind_addr));

	tracing::info!(%panel_bind_addr, "vane started");

	tokio::signal::ctrl_c().await.context("failed to listen for shutdown signal")?;
	tracing::info!("shutdown signal received");

	engine.shutdown();
	panel_task.abort();

	Ok(())
}

fn init_tracing() {
	let _ = tracing_subscriber::fmt()
		.with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
		.try_init();
}

fn panel_bind_addr() -> Result<SocketAddr> {
	std::env::var("VANE_PANEL_BIND_ADDR")
		.unwrap_or_else(|_| DEFAULT_PANEL_BIND_ADDR.to_owned())
		.parse()
		.with_context(|| "invalid VANE_PANEL_BIND_ADDR")
}

fn load_initial_config() -> Result<ConfigTable> {
	let Some(path) = std::env::var_os("VANE_CONFIG_PATH").map(PathBuf::from) else {
		tracing::info!("no VANE_CONFIG_PATH set, starting with empty config");
		return Ok(ConfigTable::default());
	};

	let raw = std::fs::read_to_string(&path)
		.with_context(|| format!("failed to read config file {}", path.display()))?;
	let config = serde_json::from_str(&raw)
		.with_context(|| format!("failed to parse config file {}", path.display()))?;
	tracing::info!(path = %path.display(), "loaded initial config");
	Ok(config)
}
