/* src/app/src/plugins/cgi/mod.rs */

pub mod executor;
pub mod stream;

use crate::l7::container::Container;
use anyhow::Result;
use async_trait::async_trait;
use executor::CgiConfig;
use serde_json::Value;
use std::{any::Any, borrow::Cow};
use vane_engine::engine::interfaces::{
	HttpMiddleware, L7Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};

pub struct CgiPlugin;

impl Plugin for CgiPlugin {
	fn name(&self) -> &str {
		"internal.driver.cgi"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			// Execution
			ParamDef { name: Cow::Borrowed("command"), required: true, param_type: ParamType::String },
			ParamDef { name: Cow::Borrowed("script"), required: false, param_type: ParamType::String },
			ParamDef { name: Cow::Borrowed("timeout"), required: false, param_type: ParamType::Integer },
			// Metadata Inputs (Template Injection Targets)
			ParamDef { name: Cow::Borrowed("method"), required: false, param_type: ParamType::String },
			ParamDef { name: Cow::Borrowed("uri"), required: true, param_type: ParamType::String },
			ParamDef { name: Cow::Borrowed("query"), required: false, param_type: ParamType::String },
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
			ParamDef { name: Cow::Borrowed("doc_root"), required: false, param_type: ParamType::String },
			ParamDef { name: Cow::Borrowed("path_info"), required: false, param_type: ParamType::String },
			ParamDef {
				name: Cow::Borrowed("script_name"),
				required: false,
				param_type: ParamType::String,
			},
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
impl HttpMiddleware for CgiPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec![Cow::Borrowed("success"), Cow::Borrowed("failure")]
	}

	async fn execute(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput> {
		let container = context
			.downcast_mut::<Container>()
			.ok_or_else(|| anyhow::anyhow!("Context is not a Container"))?;

		// Helper closure for resolving optional strings
		let get_str =
			|key: &str| -> String { inputs.get(key).and_then(Value::as_str).unwrap_or("").to_owned() };

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
			(clean_path.to_owned(), raw_query)
		} else {
			// Case B: Query is Implicit (Fallback).
			// Try to extract from URI.
			match raw_uri.split_once('?') {
				Some((path, query)) => (path.to_owned(), query.to_owned()),
				None => (raw_uri.to_owned(), String::new()),
			}
		};

		// 3. Path Info Auto Calculation
		// RFC 3875: PATH_INFO = SCRIPT_NAME prefix stripped from URI
		let mut script_name = get_str("script_name");
		let mut path_info = get_str("path_info");

		if path_info.is_empty() && !script_name.is_empty() {
			let (derived_script, derived_path) = derive_path_info(&final_uri, &script_name);
			script_name = derived_script;
			path_info = derived_path;
		}

		let config = CgiConfig {
			command,
			script: get_str("script"),
			timeout: inputs.get("timeout").and_then(Value::as_u64).unwrap_or(30),

			// Metadata resolution
			method: inputs.get("method").and_then(Value::as_str).unwrap_or("GET").to_owned(),
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

#[async_trait]
impl L7Middleware for CgiPlugin {
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

/// Robustly derives SCRIPT_NAME and PATH_INFO from a URI and a base script name.
/// Adheres to RFC 3875 by ensuring segment-based splitting.
fn derive_path_info(uri: &str, script_name: &str) -> (String, String) {
	if script_name.is_empty() {
		return (String::new(), uri.to_owned());
	}

	// Normalize slashes for robust matching

	let normalize = |p: &str| -> String {
		let mut res = String::with_capacity(p.len());

		let mut last_slash = false;

		for c in p.chars() {
			if c == '/' {
				if !last_slash {
					res.push(c);
				}

				last_slash = true;
			} else {
				res.push(c);

				last_slash = false;
			}
		}

		if !res.starts_with('/') {
			res.insert(0, '/');
		}

		res
	};

	let norm_uri = normalize(uri);

	let norm_script = normalize(script_name);

	// Remove trailing slash from script_name for matching (unless it is just "/")

	let match_base = if norm_script.len() > 1 && norm_script.ends_with('/') {
		&norm_script[..norm_script.len() - 1]
	} else {
		&norm_script
	};

	if let Some(remainder) = norm_uri.strip_prefix(match_base) {
		if remainder.is_empty() {
			// Exact match: /cgi -> SCRIPT_NAME=/cgi, PATH_INFO=""

			return (match_base.to_owned(), String::new());
		} else if remainder.starts_with('/') {
			// Segment match: /cgi/foo -> SCRIPT_NAME=/cgi, PATH_INFO=/foo

			return (match_base.to_owned(), remainder.to_owned());
		} else if match_base == "/" {
			// Root match

			return ("/".to_owned(), format!("/{}", remainder.trim_start_matches('/')));
		}
	}

	// No segment-based match: fallback

	(String::new(), norm_uri)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Tests CGI path derivation logic.
	#[test]
	fn test_derive_path_info() {
		// 1. Exact match
		assert_eq!(
			derive_path_info("/cgi-bin/script", "/cgi-bin/script"),
			("/cgi-bin/script".to_owned(), "".to_owned())
		);

		// 2. Segment match
		assert_eq!(
			derive_path_info("/cgi-bin/script/foo/bar", "/cgi-bin/script"),
			("/cgi-bin/script".to_owned(), "/foo/bar".to_owned())
		);

		// 3. Partial prefix match (should NOT match)
		// Current bug: /cgi-bin/script starts with /cgi -> PATH_INFO = -bin/script
		// Fixed behavior: should not split unless at / boundary
		assert_eq!(
			derive_path_info("/cgi-bin/script", "/cgi"),
			("".to_owned(), "/cgi-bin/script".to_owned())
		);

		// 4. Root script name
		assert_eq!(derive_path_info("/foo/bar", "/"), ("/".to_owned(), "/foo/bar".to_owned()));

		// 5. Empty script name
		assert_eq!(derive_path_info("/foo/bar", ""), ("".to_owned(), "/foo/bar".to_owned()));

		// 6. Non-matching paths
		assert_eq!(derive_path_info("/api/v1", "/cgi"), ("".to_owned(), "/api/v1".to_owned()));

		// 7. Redundant slashes
		assert_eq!(
			derive_path_info("//cgi-bin//script", "/cgi-bin"),
			("/cgi-bin".to_owned(), "/script".to_owned())
		);
	}
}
