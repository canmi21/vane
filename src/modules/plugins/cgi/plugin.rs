/* src/modules/plugins/cgi/plugin.rs */

use super::executor::{self, CgiConfig};
use crate::modules::plugins::model::{
	L7Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};
use crate::modules::stack::protocol::application::container::Container;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::{any::Any, borrow::Cow};

pub struct CgiPlugin;

impl Plugin for CgiPlugin {
	fn name(&self) -> &str {
		"internal.driver.cgi"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			// Execution
			ParamDef {
				name: Cow::Borrowed("command"),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("script"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("timeout"),
				required: false,
				param_type: ParamType::Integer,
			},
			// Metadata Inputs (Template Injection Targets)
			ParamDef {
				name: Cow::Borrowed("method"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("uri"),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("query"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("remote_addr"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("remote_port"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("server_port"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("server_name"),
				required: false,
				param_type: ParamType::String,
			},
			// Context
			ParamDef {
				name: Cow::Borrowed("doc_root"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("path_info"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("script_name"),
				required: false,
				param_type: ParamType::String,
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
impl L7Middleware for CgiPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec![Cow::Borrowed("success"), Cow::Borrowed("failure")]
	}

	async fn execute_l7(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput> {
		let container = context
			.downcast_mut::<Container>()
			.ok_or_else(|| anyhow::anyhow!("Context is not a Container"))?;

		// Helper closure for resolving optional strings
		let get_str = |key: &str| -> String {
			inputs
				.get(key)
				.and_then(Value::as_str)
				.unwrap_or("")
				.to_string()
		};

		// 1. Mandatory Fields
		let command = get_str("command");
		if command.is_empty() {
			return Err(anyhow::anyhow!("CGI: 'command' param is required"));
		}

		let raw_uri = inputs
			.get("uri")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow::anyhow!("CGI: 'uri' param is required"))?;

		// 2. URI & Query Parsing Logic
		let raw_query = get_str("query");

		let (final_uri, final_query) = if !raw_query.is_empty() {
			// Case A: Query is Explicit.
			// We strip '?' from URI if present to ensure 'uri' is pure Path.
			let clean_path = raw_uri.split_once('?').map(|(p, _)| p).unwrap_or(raw_uri);
			(clean_path.to_string(), raw_query)
		} else {
			// Case B: Query is Implicit (Fallback).
			// Try to extract from URI.
			match raw_uri.split_once('?') {
				Some((path, query)) => (path.to_string(), query.to_string()),
				None => (raw_uri.to_string(), String::new()),
			}
		};

		// 3. Path Info Auto Calculation
		// RFC 3875: PATH_INFO = SCRIPT_NAME prefix stripped from URI
		let mut path_info = get_str("path_info");
		let script_name = get_str("script_name");

		if path_info.is_empty() && !script_name.is_empty() && final_uri.starts_with(&script_name) {
			path_info = final_uri[script_name.len()..].to_string();
		}

		let config = CgiConfig {
			command,
			script: get_str("script"),
			timeout: inputs.get("timeout").and_then(Value::as_u64).unwrap_or(30),

			// Metadata resolution
			method: inputs
				.get("method")
				.and_then(Value::as_str)
				.unwrap_or("GET")
				.to_string(),
			uri: final_uri,
			query: final_query,
			remote_addr: get_str("remote_addr"),
			remote_port: get_str("remote_port"),
			server_port: get_str("server_port"),
			server_name: get_str("server_name"),

			// Script Context
			doc_root: get_str("doc_root"),
			path_info, // Use the calculated path_info
			script_name,
		};

		executor::execute(container, config).await
	}
}
