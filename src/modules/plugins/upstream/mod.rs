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
				name: "query".into(), // Optional: overrides or appends query string
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

		// 2. Resolve Path & Query
		// Logic:
		// - If 'query' input is present: Use it, and strip any '?' from the path.
		// - If 'query' input is missing:
		//    - Check if 'path' input has '?' embedded.
		//    - If 'path' input is missing (default mode), use 'req.path' AND 'req.query'.

		let path_input = inputs.get("path").and_then(Value::as_str);
		let query_input = inputs.get("query").and_then(Value::as_str);

		let raw_path = if let Some(p) = path_input {
			p.to_string()
		} else {
			container
				.kv
				.get("req.path")
				.cloned()
				.unwrap_or_else(|| "/".to_string())
		};

		let (clean_path, final_query) = if let Some(q) = query_input {
			// Case A: Explicit Query provided.
			// Strip '?' from path to avoid duplication/confusion.
			let p = raw_path
				.split_once('?')
				.map(|(pre, _)| pre)
				.unwrap_or(&raw_path);
			(p.to_string(), q.to_string())
		} else {
			// Case B: No Explicit Query provided.
			if let Some((p, q)) = raw_path.split_once('?') {
				// Sub-case B1: Query embedded in 'path' input (e.g. "/foo?bar=baz")
				(p.to_string(), q.to_string())
			} else {
				// Sub-case B2: No query in path.
				// If we are in "Default Mode" (path_input is None), we should try to carry over the original query.
				if path_input.is_none() {
					let q = container.kv.get("req.query").cloned().unwrap_or_default();
					(raw_path, q)
				} else {
					// User specified a path without query, and no explicit query input. Assume no query.
					(raw_path, String::new())
				}
			}
		};

		let path_normalized = clean_path.trim_start_matches('/'); // Normalize: remove leading slash

		// 3. Construct Full URL
		// Logic: {prefix}/{path}?{query}
		let full_url = if final_query.is_empty() {
			format!("{}/{}", url_prefix, path_normalized)
		} else {
			format!("{}/{}?{}", url_prefix, path_normalized, final_query)
		};

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
			// Passed all 4 required arguments to match quinn_client signature
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
