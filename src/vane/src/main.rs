use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use vane_engine::config::ConfigTable;
use vane_engine::engine::Engine;
use vane_engine::flow::default_plugin_registry;
use vane_transport::tls::CertStore;

#[tokio::main]
async fn main() -> Result<()> {
	init_tracing();

	let initial_config = load_initial_config()?;

	let mut engine = Engine::new(initial_config, default_plugin_registry(), CertStore::new())
		.context("failed to build engine")?;
	engine.start().await.context("failed to start engine")?;

	let engine = Arc::new(engine);

	tracing::info!("vane started");

	tokio::signal::ctrl_c().await.context("failed to listen for shutdown signal")?;
	tracing::info!("shutdown signal received");

	engine.shutdown();

	Ok(())
}

fn init_tracing() {
	let _ = tracing_subscriber::fmt()
		.with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
		.try_init();
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
