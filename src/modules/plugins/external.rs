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
use fancy_log::{LogLevel, log};
use std::any::Any;
use std::borrow::Cow;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A wrapper struct that implements the Plugin trait for external definitions.
#[derive(Debug, Clone)]
pub struct ExternalPlugin {
	config: ExternalPluginConfig,
}

impl ExternalPlugin {
	pub fn new(config: ExternalPluginConfig) -> Self {
		Self { config }
	}

	/// Checks if a program exists in the system PATH or at a specific path.
	fn find_program(program: &str) -> Option<PathBuf> {
		let path = Path::new(program);
		if path.components().count() > 1 {
			if path.exists() {
				return Some(path.to_path_buf());
			}
			return None;
		}
		if let Ok(path_var) = env::var("PATH") {
			for path_entry in env::split_paths(&path_var) {
				let p = path_entry.join(program);
				if p.exists() {
					return Some(p);
				}
			}
		}
		None
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

		if skip_validation {
			log(
				LogLevel::Debug,
				&format!(
					"⚠ Skipping connectivity validation for plugin '{}'.",
					self.name()
				),
			);
			return Ok(());
		}

		match &self.config.driver {
			ExternalPluginDriver::Http { url } => {
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
				if !Path::new(path).exists() {
					return Err(anyhow!("Unix socket path does not exist: {}", path));
				}
				Ok(())
			}
			ExternalPluginDriver::Command { program, .. } => {
				if Self::find_program(program).is_none() {
					return Err(anyhow!(
						"Program '{}' not found in PATH or at specified location.",
						program
					));
				}
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

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		None
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
		_kv: &KvStore,
		_conn: ConnectionObject,
	) -> Result<TerminatorResult> {
		Err(anyhow!(
			"Execution Error: External plugins cannot be executed as Terminators."
		))
	}
}
