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
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;

#[derive(Debug, Clone)]
pub struct ExternalPlugin {
	config: ExternalPluginConfig,
}

pub async fn get_trusted_bin_root() -> PathBuf {
	let root = crate::common::getconf::get_config_dir().join("bin");
	fs::canonicalize(&root).await.unwrap_or(root)
}

pub async fn validate_command_path(program: &str) -> Result<PathBuf> {
	let bin_root = get_trusted_bin_root().await;
	let program_path = Path::new(program);

	let absolute_path = if program_path.is_absolute() {
		fs::canonicalize(program_path)
			.await
			.map_err(|e| anyhow!("Failed to resolve absolute path '{}': {}", program, e))?
	} else {
		let joined = bin_root.join(program_path);
		fs::canonicalize(&joined).await.map_err(|e| {
			anyhow!(
				"Program '{}' not found in trusted bin directory: {}",
				program,
				e
			)
		})?
	};

	if !absolute_path.starts_with(&bin_root) {
		return Err(anyhow!(
			"Security Violation - Program '{}' is outside the trusted bin directory.",
			program
		));
	}

	if !fs::metadata(&absolute_path)
		.await
		.map(|m| m.is_file())
		.unwrap_or(false)
	{
		return Err(anyhow!("Path '{}' is not a file.", program));
	}

	Ok(absolute_path)
}

impl ExternalPlugin {
	pub fn new(config: ExternalPluginConfig) -> Self {
		Self { config }
	}

	pub async fn validate_connectivity(&self) -> Result<()> {
		if self.config.role == PluginRole::Terminator {
			return Err(anyhow!("External plugins cannot be Terminators."));
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
				let client = reqwest::Client::builder()
					.timeout(Duration::from_secs(3))
					.build()?;
				let response = client.request(reqwest::Method::OPTIONS, url).send().await?;
				if !response.status().is_success() {
					return Err(anyhow!("Endpoint returned error: {}.", response.status()));
				}
				Ok(())
			}
			ExternalPluginDriver::Unix { path } => {
				if skip_validation {
					return Ok(());
				}
				if fs::metadata(path).await.is_err() {
					return Err(anyhow!("Unix socket path does not exist: {}", path));
				}
				Ok(())
			}
			ExternalPluginDriver::Command { program, .. } => {
				validate_command_path(program).await?;
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
		vec![]
	}
	async fn execute(
		&self,
		_i: ResolvedInputs,
		_kv: &mut KvStore,
		_c: ConnectionObject,
	) -> Result<TerminatorResult> {
		Err(anyhow!("External plugins cannot be Terminators."))
	}
}
