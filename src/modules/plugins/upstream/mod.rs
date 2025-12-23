/* src/modules/plugins/upstream/mod.rs */

pub mod hyper_client;
pub mod pool;
pub mod quic_pool;
pub mod quinn_client;
pub mod tls_verifier;

use crate::modules::{
	plugins::model::{L7Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs},
	stack::protocol::application::container::Container,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use serde_json::Value;
use std::{any::Any, borrow::Cow};

pub struct FetchUpstreamPlugin;

impl Plugin for FetchUpstreamPlugin {
	fn name(&self) -> &'static str {
		"internal.driver.upstream"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "url_prefix".into(),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "path".into(), // Optional: overrides request path
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "method".into(),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "version".into(),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "skip_verify".into(),
				required: false,
				param_type: ParamType::Boolean,
			},
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_l7_middleware(&self) -> Option<&dyn L7Middleware> {
		Some(self)
	}
}

#[async_trait]
impl L7Middleware for FetchUpstreamPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["success".into(), "failure".into()]
	}

	async fn execute_l7(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput> {
		// Downcast Context to Container
		let container = context
			.downcast_mut::<Container>()
			.ok_or_else(|| anyhow!("Context is not a Container"))?;

		// 1. Resolve URL Prefix
		let url_prefix = inputs
			.get("url_prefix")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Input 'url_prefix' is required"))?
			.trim_end_matches('/'); // Normalize: remove trailing slash

		// 2. Resolve Path
		// Priority: Input 'path' > Container 'req.path' > Empty
		let path_input = inputs.get("path").and_then(Value::as_str);

		let raw_path = if let Some(p) = path_input {
			p.to_string()
		} else {
			container
				.kv
				.get("req.path")
				.cloned()
				.unwrap_or_else(|| "/".to_string())
		};

		let path_normalized = raw_path.trim_start_matches('/'); // Normalize: remove leading slash

		// 3. Construct Full URL
		// Logic: {prefix}/{path}
		let full_url = format!("{}/{}", url_prefix, path_normalized);

		let method = inputs.get("method").and_then(Value::as_str);

		let version = inputs
			.get("version")
			.and_then(Value::as_str)
			.unwrap_or("auto");

		let skip_verify = inputs
			.get("skip_verify")
			.and_then(Value::as_bool)
			.unwrap_or(false);

		log(LogLevel::Debug, &format!("➜ Upstream Target: {}", full_url));

		let result = match version {
			"auto" | "h1" | "h1.1" | "h2" => {
				hyper_client::execute_hyper_request(
					container,
					&full_url,
					method,
					Some(version),
					skip_verify,
				)
				.await
			}
			// FIXED: Passed all 4 required arguments to match quinn_client signature
			"h3" => quinn_client::execute_quinn_request(container, &full_url, method, skip_verify).await,
			_ => {
				log(
					LogLevel::Warn,
					&format!("⚠ Unknown version '{}', falling back to auto.", version),
				);
				hyper_client::execute_hyper_request(container, &full_url, method, Some("auto"), skip_verify)
					.await
			}
		};

		match result {
			Ok(_) => Ok(MiddlewareOutput {
				branch: "success".into(),
				store: None,
			}),
			Err(e) => {
				log(LogLevel::Error, &format!("FetchUpstream Failed: {}", e));
				Ok(MiddlewareOutput {
					branch: "failure".into(),
					store: Some(std::collections::HashMap::from([(
						"error".to_string(),
						e.to_string(),
					)])),
				})
			}
		}
	}
}
