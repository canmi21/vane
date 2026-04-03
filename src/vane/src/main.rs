use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use vane_engine::config::ConfigTable;
use vane_engine::engine::Engine;
use vane_engine::flow::default_plugin_registry;
use vane_panel::{PanelState, panel_bind_addr, start_panel_server};
use vane_transport::tls::CertStore;

#[tokio::main]
async fn main() -> Result<()> {
	init_tracing();

	let started_at = SystemTime::now();
	let panel_addr = resolve_panel_addr()?;
	let initial_config = load_initial_config()?;

	let engine = Engine::new(initial_config, default_plugin_registry(), CertStore::new())
		.context("failed to build engine")?;
	engine.start().await.context("failed to start engine")?;

	let engine = Arc::new(engine);
	let state = PanelState::new(Arc::clone(&engine), started_at);
	let panel_task = tokio::spawn(start_panel_server(state, panel_addr));

	tracing::info!(%panel_addr, "vane started");

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

fn resolve_panel_addr() -> Result<SocketAddr> {
	panel_bind_addr().map_err(|e| anyhow::anyhow!(e))
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
