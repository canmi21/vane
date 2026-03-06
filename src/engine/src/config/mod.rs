/* src/engine/src/config/mod.rs */

//! Unified configuration management using live crate.

use fancy_log::{LogLevel, log};
use live::controller::{KeyPattern, Live, LiveDir, LiveError, ScanMode};
use live::holder::Store;
use live::loader::{DynLoader, FileSource, format::AnyFormat};
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

mod types;
pub use types::*;

pub static CONFIG: OnceLock<ConfigManager> = OnceLock::new();

#[must_use]
pub fn get() -> &'static ConfigManager {
	CONFIG.get().expect("ConfigManager not initialized")
}

/// Manages TCP and UDP listener configurations
pub struct ListenerManager {
	pub tcp: LiveDir<TcpConfig>,
	pub udp: LiveDir<UdpConfig>,
}

impl ListenerManager {
	pub async fn init(config_dir: &Path) -> Result<Self, LiveError> {
		let tcp_store = Arc::new(Store::new());
		let udp_store = Arc::new(Store::new());

		let listener_path = config_dir.join("listener");

		// Helper to build a loader
		let build_loader = || {
			DynLoader::builder()
				.source(FileSource::new(listener_path.to_str().expect("listener path is valid UTF-8")))
				.format(AnyFormat::Toml)
				.format(AnyFormat::Yaml)
				.format(AnyFormat::Json)
				.build()
				.map_err(|e| LiveError::Builder(e.to_owned()))
		};

		// TCP configs: listener/[port]/tcp.toml
		let tcp = LiveDir::builder()
			.store(tcp_store)
			.loader(build_loader()?)
			.path(&listener_path)
			.pattern(KeyPattern::Bracketed)
			.scan_mode(ScanMode::Subdirs { config_file: "tcp".to_owned() })
			.on_error(|e| {
				log(
					LogLevel::Warn,
					&format!("✗ New TCP config is invalid. Keeping last known good version. Error: {e}"),
				);
			})
			.build()?;

		// UDP configs: listener/[port]/udp.toml
		let udp = LiveDir::builder()
			.store(udp_store)
			.loader(build_loader()?)
			.path(&listener_path)
			.pattern(KeyPattern::Bracketed)
			.scan_mode(ScanMode::Subdirs { config_file: "udp".to_owned() })
			.on_error(|e| {
				log(
					LogLevel::Warn,
					&format!("✗ New UDP config is invalid. Keeping last known good version. Error: {e}"),
				);
			})
			.build()?;

		Ok(Self { tcp, udp })
	}

	#[must_use]
	pub fn get_tcp(&self, port: &str) -> Option<Arc<TcpConfig>> {
		self.tcp.get(port)
	}

	#[must_use]
	pub fn get_udp(&self, port: &str) -> Option<Arc<UdpConfig>> {
		self.udp.get(port)
	}

	pub async fn load(&self) -> Result<(), LiveError> {
		self.tcp.load().await?;
		self.udp.load().await?;
		Ok(())
	}

	pub async fn start_watching(&mut self, config: live::signal::Config) -> Result<(), LiveError> {
		self.tcp.start_watching(config.clone()).await?;
		self.udp.start_watching(config).await?;
		Ok(())
	}
}

/// Global configuration manager
pub struct ConfigManager {
	pub listeners: ListenerManager,
	pub resolvers: LiveDir<ResolverConfig>,
	pub applications: LiveDir<ApplicationConfig>,
	pub nodes: Live<NodesConfig>,
	pub lazycert: Option<Live<LazyCertConfig>>,
}

impl ConfigManager {
	pub async fn init(config_dir_str: &str) -> Result<Self, LiveError> {
		let config_dir = Path::new(config_dir_str);

		let build_loader = || {
			DynLoader::builder()
				.source(FileSource::new(config_dir_str))
				.format(AnyFormat::Toml)
				.format(AnyFormat::Yaml)
				.format(AnyFormat::Json)
				.build()
				.map_err(|e| LiveError::Builder(e.to_owned()))
		};

		let listeners = ListenerManager::init(config_dir).await?;

		// Resolvers
		let resolver_path = config_dir.join("resolver");
		let resolvers = LiveDir::builder()
			.store(Arc::new(Store::new()))
			.loader(
				DynLoader::builder()
					.source(FileSource::new(resolver_path.to_str().expect("resolver path is valid UTF-8")))
					.format(AnyFormat::Toml)
					.format(AnyFormat::Yaml)
					.format(AnyFormat::Json)
					.build()
					.map_err(|e| LiveError::Builder(e.to_owned()))?,
			)
			.path(&resolver_path)
			.pattern(KeyPattern::Identity)
			.scan_mode(ScanMode::Files)
			.on_error(|e| {
				log(
					LogLevel::Warn,
					&format!("✗ Resolver config reload failed. Keeping last known good version. Error: {e}"),
				);
			})
			.build()?;

		// Applications
		let application_path = config_dir.join("application");
		let applications = LiveDir::builder()
			.store(Arc::new(Store::new()))
			.loader(
				DynLoader::builder()
					.source(FileSource::new(
						application_path.to_str().expect("application path is valid UTF-8"),
					))
					.format(AnyFormat::Toml)
					.format(AnyFormat::Yaml)
					.format(AnyFormat::Json)
					.build()
					.map_err(|e| LiveError::Builder(e.to_owned()))?,
			)
			.path(&application_path)
			.pattern(KeyPattern::Identity)
			.scan_mode(ScanMode::Files)
			.on_error(|e| {
				log(
					LogLevel::Warn,
					&format!(
						"✗ Application config reload failed. Keeping last known good version. Error: {e}"
					),
				);
			})
			.build()?;
		// Nodes
		let nodes = Live::new(Arc::new(Store::new()), build_loader()?, "nodes");

		// LazyCert (optional)
		let lazycert_live = Live::new(Arc::new(Store::new()), build_loader()?, "lazycert");

		Ok(Self { listeners, resolvers, applications, nodes, lazycert: Some(lazycert_live) })
	}
}
