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
use http::HeaderValue;
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
				name: "path".into(),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "query".into(),
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
			ParamDef {
				name: "websocket".into(),
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

		// 0. WebSocket Gatekeeper Logic
		let websocket_enabled = inputs
			.get("websocket")
			.and_then(Value::as_bool)
			.unwrap_or(false);
		let is_upgrade_request = container.client_upgrade.is_some();

		if is_upgrade_request && !websocket_enabled {
			// Case: Upgrade requested but NOT enabled -> 405 Method Not Allowed
			log(LogLevel::Warn, "✗ Rejected WebSocket/Upgrade request.");

			container
				.kv
				.insert("res.status".to_string(), "405".to_string());
			container
				.response_headers
				.insert(http::header::ALLOW, HeaderValue::from_static("GET"));
			container
				.response_headers
				.insert(http::header::CONNECTION, HeaderValue::from_static("close"));
			// Ensure body is empty
			container.response_body =
				crate::modules::stack::protocol::application::container::PayloadState::Empty;

			// Return SUCCESS branch so the Terminator can send the 405 response properly.
			// Returning FAILURE would abort the connection without sending the response.
			return Ok(MiddlewareOutput {
				branch: "success".into(),
				store: None,
			});
		}

		// 1. Resolve URL Prefix
		let url_prefix = inputs
			.get("url_prefix")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Input 'url_prefix' is required"))?
			.trim_end_matches('/');

		// 2. Resolve Path & Query
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
			let p = raw_path
				.split_once('?')
				.map(|(pre, _)| pre)
				.unwrap_or(&raw_path);
			(p.to_string(), q.to_string())
		} else {
			if let Some((p, q)) = raw_path.split_once('?') {
				(p.to_string(), q.to_string())
			} else {
				if path_input.is_none() {
					let q = container.kv.get("req.query").cloned().unwrap_or_default();
					(raw_path, q)
				} else {
					(raw_path, String::new())
				}
			}
		};

		let path_normalized = clean_path.trim_start_matches('/');

		// 3. Construct Full URL
		let full_url = if final_query.is_empty() {
			format!("{}/{}", url_prefix, path_normalized)
		} else {
			format!("{}/{}?{}", url_prefix, path_normalized, final_query)
		};

		let method = inputs.get("method").and_then(Value::as_str);
		let skip_verify = inputs
			.get("skip_verify")
			.and_then(Value::as_bool)
			.unwrap_or(false);

		// 4. Version Selection Logic (Force H1 for Websockets)
		let config_version = inputs
			.get("version")
			.and_then(Value::as_str)
			.unwrap_or("auto");

		let final_version = if is_upgrade_request && websocket_enabled {
			log(LogLevel::Debug, "⚙ Forcing HTTP/1.1 for WebSocket Upgrade.");
			"h1" // Force H1
		} else {
			config_version
		};

		log(LogLevel::Debug, &format!("➜ Upstream Target: {}", full_url));

		let result = match final_version {
			"auto" | "h1" | "h1.1" | "h2" => {
				hyper_client::execute_hyper_request(
					container,
					&full_url,
					method,
					Some(final_version),
					skip_verify,
				)
				.await
			}
			"h3" => quinn_client::execute_quinn_request(container, &full_url, method, skip_verify).await,
			_ => {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Unknown version '{}', falling back to auto.",
						final_version
					),
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
