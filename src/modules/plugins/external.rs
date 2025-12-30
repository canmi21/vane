/* src/modules/plugins/external.rs */

use super::{
	drivers,
	model::{
		ExternalPluginConfig, ExternalPluginDriver, Layer, Middleware, MiddlewareOutput, ParamDef,
		ParamType, Plugin, PluginRole, ResolvedInputs, Terminator, TerminatorResult,
	},
};
use crate::common::getenv;
use crate::modules::kv::KvStore;
use crate::modules::plugins::model::ConnectionObject;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::any::Any;
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A wrapper struct that implements the Plugin trait for external definitions.
#[derive(Debug, Clone)]
pub struct ExternalPlugin {
	config: ExternalPluginConfig,
}

/// Resolves the trusted plugin bin directory from the configuration.
pub fn get_trusted_bin_root() -> PathBuf {
	let root = crate::common::getconf::get_config_dir().join("bin");
	// Use canonicalize to resolve symlinks and ensure we have an absolute path.
	// If it doesn't exist yet, fallback to the joined path.
	fs::canonicalize(&root).unwrap_or(root)
}

/// Validates that a program path is safe and located within the trusted bin directory.
/// Returns the absolute path to the program if valid.
pub fn validate_command_path(program: &str) -> Result<PathBuf> {
	let bin_root = get_trusted_bin_root();
	let program_path = Path::new(program);

	// Scenario 1: Relative path or filename -> join with bin_root
	let absolute_path = if program_path.is_absolute() {
		// Scenario 2: Absolute path -> must be canonicalized and checked for prefix
		fs::canonicalize(program_path).map_err(|e| {
			anyhow!(
				"SEC-2: Failed to resolve absolute path '{}': {}",
				program,
				e
			)
		})?
	} else {
		// Join and then canonicalize to resolve any ".."
		let joined = bin_root.join(program_path);
		fs::canonicalize(&joined).map_err(|e| {
			anyhow!(
				"SEC-2: Program '{}' not found in trusted bin directory: {}",
				program,
				e
			)
		})?
	};

	// Strict prefix check
	if !absolute_path.starts_with(&bin_root) {
		return Err(anyhow!(
			"SEC-2: Security Violation - Program '{}' is outside the trusted bin directory.",
			program
		));
	}

	if !absolute_path.is_file() {
		return Err(anyhow!("SEC-2: Path '{}' is not a file.", program));
	}

	Ok(absolute_path)
}

impl ExternalPlugin {
	pub fn new(config: ExternalPluginConfig) -> Self {
		Self { config }
	}

	pub async fn validate_connectivity(&self) -> Result<()> {
		if self.config.role == PluginRole::Terminator {
			return Err(anyhow!(
				"External plugins cannot be Terminators. Only built-in plugins can terminate connections."
			));
		}

		let skip_validation = getenv::to_lowercase(&getenv::get_env(
			"SKIP_VALIDATE_CONNECTIVITY",
			"false".to_string(),
		)) == "true";

		match &self.config.driver {
			ExternalPluginDriver::Http { url } => {
				if skip_validation {
					return Ok(());
				}
				if !url.starts_with("http://") && !url.starts_with("https://") {
					return Err(anyhow!("URL must start with http:// or https://"));
				}
				let client = reqwest::Client::builder()
					.timeout(Duration::from_secs(3))
					.build()
					.map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;
				let response = client
					.request(reqwest::Method::OPTIONS, url)
					.send()
					.await
					.map_err(|e| anyhow!("Failed to connect to endpoint: {}", e))?;
				if !response.status().is_success() {
					return Err(anyhow!(
						"Endpoint returned error status: {}.",
						response.status()
					));
				}
				Ok(())
			}
			ExternalPluginDriver::Unix { path } => {
				if skip_validation {
					return Ok(());
				}
				if !Path::new(path).exists() {
					return Err(anyhow!("Unix socket path does not exist: {}", path));
				}
				Ok(())
			}
			ExternalPluginDriver::Command { program, .. } => {
				// Command validation cannot be fully skipped as it is a core security feature (SEC-2)
				validate_command_path(program)?;
				Ok(())
			}
		}
	}
}

impl Plugin for ExternalPlugin {
	fn name(&self) -> &str {
		&self.config.name
	}

	fn params(&self) -> Vec<ParamDef> {
		self
			.config
			.params
			.iter()
			.map(|p| ParamDef {
				name: Cow::Owned(p.name.clone()),
				required: p.required,
				param_type: ParamType::String,
			})
			.collect()
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		if self.config.role == PluginRole::Middleware {
			Some(self)
		} else {
			None
		}
	}

	fn as_generic_middleware(&self) -> Option<&dyn super::model::GenericMiddleware> {
		if self.config.role == PluginRole::Middleware {
			Some(self)
		} else {
			None
		}
	}

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		None
	}
}

#[async_trait]
impl super::model::GenericMiddleware for ExternalPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["success".into(), "failure".into()]
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		drivers::execute_driver(&self.config.driver, self.name(), inputs).await
	}
}

#[async_trait]
impl Middleware for ExternalPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["success".into(), "failure".into()]
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		drivers::execute_driver(&self.config.driver, self.name(), inputs).await
	}
}

#[async_trait]
impl Terminator for ExternalPlugin {
	fn supported_layers(&self) -> Vec<Layer> {
		vec![] // Not supported
	}

	async fn execute(
		&self,
		_inputs: ResolvedInputs,
		_kv: &mut KvStore,
		_conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		Err(anyhow!(
			"Execution Error: External plugins cannot be executed as Terminators."
		))
	}
}
