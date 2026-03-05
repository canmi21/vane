pub mod hyper_client;
pub mod pool;
#[cfg(feature = "h3upstream")]
pub mod quic_pool;
#[cfg(feature = "h3upstream")]
pub mod quinn_client;
pub mod tls_verifier;

use crate::l7::container::{Container, PayloadState};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fancy_log::{LogLevel, log};
use http::HeaderValue;
use serde_json::Value;
use std::{any::Any, borrow::Cow};
use vane_engine::engine::interfaces::{
	HttpMiddleware, L7Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};

pub struct FetchUpstreamPlugin;

impl Plugin for FetchUpstreamPlugin {
	fn name(&self) -> &'static str {
		"internal.driver.upstream"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef { name: "url_prefix".into(), required: true, param_type: ParamType::String },
			ParamDef { name: "path".into(), required: false, param_type: ParamType::String },
			ParamDef { name: "query".into(), required: false, param_type: ParamType::String },
			ParamDef { name: "method".into(), required: false, param_type: ParamType::String },
			ParamDef { name: "version".into(), required: false, param_type: ParamType::String },
			ParamDef { name: "skip_verify".into(), required: false, param_type: ParamType::Boolean },
			ParamDef { name: "websocket".into(), required: false, param_type: ParamType::Boolean },
		]
	}

	fn supported_protocols(&self) -> Vec<Cow<'static, str>> {
		vec!["httpx".into()]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_http_middleware(&self) -> Option<&dyn HttpMiddleware> {
		Some(self)
	}

	fn as_l7_middleware(&self) -> Option<&dyn L7Middleware> {
		Some(self)
	}
}

#[async_trait]
impl HttpMiddleware for FetchUpstreamPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["success".into(), "failure".into()]
	}

	async fn execute(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput> {
		let container =
			context.downcast_mut::<Container>().ok_or_else(|| anyhow!("Context is not a Container"))?;

		let is_client_ws_upgrade =
			container.http_data().and_then(|d| d.client_upgrade.as_ref()).is_some();
		let websocket_enabled = inputs.get("websocket").and_then(Value::as_bool).unwrap_or(false);

		// Handle WebSocket Upgrade requests
		if is_client_ws_upgrade {
			if !websocket_enabled {
				// Client wants WebSocket but config disallows it
				// Generate 405 response internally and return success
				log(LogLevel::Warn, "⚠ WebSocket upgrade requested but not allowed by config.");

				container.kv.insert("res.status".to_owned(), "405".to_owned());
				container.kv.insert(
					"res.body".to_owned(),
					"Method Not Allowed: WebSocket upgrade is disabled".to_owned(),
				);

				// Set response headers
				container
					.response_headers
					.insert(http::header::CONNECTION, HeaderValue::from_static("close"));
				container.response_headers.insert(
					http::header::CONTENT_TYPE,
					HeaderValue::from_static("text/plain; charset=utf-8"),
				);

				// Body will be populated by SendResponse from KV
				container.response_body = PayloadState::Empty;

				return Ok(MiddlewareOutput {
					branch: "success".into(),
					store: Some(std::collections::HashMap::from([(
						"error".to_owned(),
						"WebSocket not allowed".to_owned(),
					)])),
				});
			}

			// WebSocket upgrade allowed, proceed with H1.1 request
			log(LogLevel::Debug, "⚙ WebSocket upgrade detected, forcing HTTP/1.1");
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
			p.to_owned()
		} else {
			container.kv.get("req.path").cloned().unwrap_or_else(|| "/".to_owned())
		};

		let (clean_path, final_query) = if let Some(q) = query_input {
			let p = raw_path.split_once('?').map(|(pre, _)| pre).unwrap_or(&raw_path);
			(p.to_owned(), q.to_owned())
		} else if let Some((p, q)) = raw_path.split_once('?') {
			(p.to_owned(), q.to_owned())
		} else if path_input.is_none() {
			let q = container.kv.get("req.query").cloned().unwrap_or_default();
			(raw_path, q)
		} else {
			(raw_path, String::new())
		};

		let path_normalized = clean_path.trim_start_matches('/');

		// 3. Construct Full URL
		let full_url = if final_query.is_empty() {
			format!("{url_prefix}/{path_normalized}")
		} else {
			format!("{url_prefix}/{path_normalized}?{final_query}")
		};

		let method = inputs.get("method").and_then(Value::as_str);

		let version = inputs.get("version").and_then(Value::as_str).unwrap_or("auto");

		let skip_verify = inputs.get("skip_verify").and_then(Value::as_bool).unwrap_or(false);

		log(LogLevel::Debug, &format!("➜ Upstream Target: {full_url}"));

		// For WebSocket upgrade requests, always use H1.1 regardless of version config
		let result = if is_client_ws_upgrade && websocket_enabled {
			hyper_client::execute_h1_websocket_request(container, &full_url, method, skip_verify).await
		} else {
			// Normal HTTP request, respect version config
			match version {
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
				#[cfg(feature = "h3upstream")]
				"h3" => quinn_client::execute_quinn_request(container, &full_url, method, skip_verify).await,
				#[cfg(not(feature = "h3upstream"))]
				"h3" => {
					return Err(anyhow!("HTTP/3 upstream support is disabled in this build."));
				}
				_ => {
					log(LogLevel::Warn, &format!("⚠ Unknown version '{version}', falling back to auto."));
					hyper_client::execute_hyper_request(
						container,
						&full_url,
						method,
						Some("auto"),
						skip_verify,
					)
					.await
				}
			}
		};

		match result {
			Ok(_) => Ok(MiddlewareOutput { branch: "success".into(), store: None }),
			Err(e) => {
				log(LogLevel::Error, &format!("✗ FetchUpstream Failed: {e}"));
				Ok(MiddlewareOutput {
					branch: "failure".into(),
					store: Some(std::collections::HashMap::from([("error".to_owned(), e.to_string())])),
				})
			}
		}
	}
}

#[async_trait]
impl L7Middleware for FetchUpstreamPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		<Self as HttpMiddleware>::output(self)
	}

	async fn execute_l7(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput> {
		<Self as HttpMiddleware>::execute(self, context, inputs).await
	}
}
