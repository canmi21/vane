/* src/modules/plugins/external.rs */

use super::model::{
	ExternalPluginConfig, ExternalPluginDriver, Middleware, MiddlewareOutput, ParamDef, ParamType,
	Plugin, PluginRole, ResolvedInputs, Terminator,
};
use crate::common::getenv;
use crate::modules::kv::KvStore;
use crate::modules::plugins::model::ConnectionObject;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use std::any::Any;
use std::path::Path;
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

	/// Validates that the external resource exists or is reachable.
	/// This is called during registration.
	pub async fn validate_connectivity(&self) -> Result<()> {
		// Mandate: External plugins can ONLY be Middleware.
		if self.config.role == PluginRole::Terminator {
			return Err(anyhow!(
				"External plugins cannot be Terminators. Only built-in plugins can terminate connections."
			));
		}

		// Check if validation should be skipped via env var
		let skip_validation = getenv::to_lowercase(&getenv::get_env(
			"SKIP_VALIDATE_CONNECTIVITY",
			"false".to_string(),
		)) == "true";

		if skip_validation {
			log(
				LogLevel::Debug,
				&format!(
					"⚠ Skipping connectivity validation for plugin '{}' due to SKIP_VALIDATE_CONNECTIVITY env.",
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

				// Create a transient HTTP client for validation
				let client = reqwest::Client::builder()
					.timeout(Duration::from_secs(3)) // Fail fast during registration (3s)
					.build()
					.map_err(|e| anyhow!("Failed to build HTTP client for validation: {}", e))?;

				// Send an OPTIONS request to check connectivity and readiness
				let response = client
					.request(reqwest::Method::OPTIONS, url)
					.send()
					.await
					.map_err(|e| anyhow!("Failed to connect to external plugin endpoint: {}", e))?;

				// We expect a success status code (200-299) to confirm the service is ready.
				if !response.status().is_success() {
					return Err(anyhow!(
						"External plugin endpoint returned error status: {}. Expected 2xx.",
						response.status()
					));
				}

				log(
					LogLevel::Debug,
					&format!(
						"✓ External plugin '{}' connectivity verified via OPTIONS.",
						self.name()
					),
				);
				Ok(())
			}
			ExternalPluginDriver::Unix { path } => {
				if !Path::new(path).exists() {
					return Err(anyhow!("Unix socket path does not exist: {}", path));
				}
				Ok(())
			}
			ExternalPluginDriver::Bin { path, .. } => {
				if !Path::new(path).exists() {
					return Err(anyhow!("Binary executable not found at: {}", path));
				}
				// Check for execute permissions on Unix systems could go here.
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
		// Map external param defs to internal ParamDef
		self
			.config
			.params
			.iter()
			.map(|p| ParamDef {
				// Note: We leak memory here because ParamDef requires static str,
				// but since plugins are long-lived, this is acceptable for now.
				// A better refactor would change ParamDef to use String or Cow.
				name: Box::leak(p.name.clone().into_boxed_str()),
				required: p.required,
				param_type: ParamType::String, // Defaulting to String for external APIs
			})
			.collect()
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		// Since we enforce strict validation in `validate_connectivity`,
		// we can confidently return self here if the config matches.
		if self.config.role == PluginRole::Middleware {
			Some(self)
		} else {
			None
		}
	}

	fn as_terminator(&self) -> Option<&dyn Terminator> {
		// Explicitly disable Terminator support for External Plugins.
		None
	}
}

#[async_trait]
impl Middleware for ExternalPlugin {
	fn output(&self) -> Vec<&'static str> {
		// External middleware conventionally has "success" and "failure"
		vec!["success", "failure"]
	}

	async fn execute(&self, _inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		// TODO: Implement actual driver invocation (HTTP/Unix/Bin) using POST.
		// Runtime execution does NOT perform the OPTIONS check again.
		log(
			LogLevel::Debug,
			&format!("➜ Executing external middleware: {}", self.name()),
		);
		Ok(MiddlewareOutput {
			branch: "success",
			write_to_kv: None,
		})
	}
}

#[async_trait]
impl Terminator for ExternalPlugin {
	async fn execute(
		&self,
		_inputs: ResolvedInputs,
		_kv: &KvStore,
		_conn: ConnectionObject,
	) -> Result<()> {
		// This should technically be unreachable due to `as_terminator` returning None
		// and `validate_connectivity` rejecting the role.
		Err(anyhow!(
			"Execution Error: External plugins cannot be executed as Terminators."
		))
	}
}
