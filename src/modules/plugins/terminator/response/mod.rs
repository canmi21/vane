/* src/modules/plugins/terminator/response/mod.rs */

pub mod content_type;

use crate::modules::{
	plugins::model::{L7Terminator, ParamDef, ParamType, Plugin, ResolvedInputs, TerminatorResult},
	stack::protocol::application::{
		container::{Container, PayloadState},
		http::wrapper::VaneBody,
	},
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use fancy_log::{LogLevel, log};
use http::{HeaderName, HeaderValue, Response, StatusCode};
use http_body_util::Full;
use serde_json::Value;
use std::any::Any;

pub struct SendResponsePlugin;

impl Plugin for SendResponsePlugin {
	fn name(&self) -> &'static str {
		"internal.terminator.response"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "status".into(),
				required: false,
				param_type: ParamType::Integer,
			},
			ParamDef {
				name: "headers".into(),
				required: false,
				param_type: ParamType::Map,
			},
			ParamDef {
				name: "body".into(), // Supports String or Map {content, encoding}
				required: false,
				param_type: ParamType::Any,
			},
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_l7_terminator(&self) -> Option<&dyn L7Terminator> {
		Some(self)
	}
}

#[async_trait]
impl L7Terminator for SendResponsePlugin {
	async fn execute_l7(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<TerminatorResult> {
		let container = context
			.downcast_mut::<Container>()
			.ok_or_else(|| anyhow!("Context is not a Container"))?;

		// 1. Determine Status Code (Priority: Input > KV > 200)
		let status_code = if let Some(s) = inputs.get("status").and_then(Value::as_u64) {
			StatusCode::from_u16(s as u16).unwrap_or(StatusCode::OK)
		} else if let Some(s) = container
			.kv
			.get("res.status")
			.and_then(|s| s.parse::<u16>().ok())
		{
			StatusCode::from_u16(s).unwrap_or(StatusCode::OK)
		} else {
			StatusCode::OK
		};

		// 2. Handle Headers (Takeover vs Inherit)
		let headers = &mut container.response_headers;

		if let Some(headers_input) = inputs.get("headers").and_then(Value::as_object) {
			// Takeover Mode: Clear inherited headers and apply config
			headers.clear();

			for (k, v) in headers_input {
				let header_name = match HeaderName::from_bytes(k.as_bytes()) {
					Ok(n) => n,
					Err(_) => continue,
				};

				match v {
					Value::String(s) => {
						if let Ok(val) = HeaderValue::from_str(s) {
							headers.insert(header_name, val);
						}
					}
					Value::Array(arr) => {
						for item in arr {
							if let Some(s) = item.as_str() {
								if let Ok(val) = HeaderValue::from_str(s) {
									headers.append(header_name.clone(), val);
								}
							}
						}
					}
					_ => {}
				}
			}
		}

		// 3. Handle Body (Takeover vs Inherit)
		if let Some(body_input) = inputs.get("body") {
			// Takeover Mode: Overwrite body
			let body_bytes = parse_body_input(body_input)?;

			// Auto-Guess Content-Type for static body if missing
			if !headers.contains_key(http::header::CONTENT_TYPE) {
				let mime = content_type::guess_mime(&body_bytes);
				headers.insert(
					http::header::CONTENT_TYPE,
					HeaderValue::from_str(mime).unwrap(),
				);
			}

			let full_body = Full::new(body_bytes);
			container.response_body = PayloadState::Http(VaneBody::Buffered(full_body));
		}

		// 4. Construct Final Response
		let mut response = Response::builder().status(status_code).body(()).unwrap();
		*response.headers_mut() = std::mem::take(headers);

		// 5. Signal Adapter
		if let Some(tx) = container.response_tx.take() {
			let _ = tx.send(response);
		} else {
			log(
				LogLevel::Warn,
				"⚠ SendResponse called but response channel is missing.",
			);
		}

		Ok(TerminatorResult::Finished)
	}
}

/// Helper to decode body input (String or Map)
fn parse_body_input(input: &Value) -> Result<Bytes> {
	match input {
		Value::String(s) => Ok(Bytes::copy_from_slice(s.as_bytes())),
		Value::Object(map) => {
			let content = map
				.get("content")
				.and_then(Value::as_str)
				.ok_or_else(|| anyhow!("Structured body missing 'content' field"))?;

			let encoding = map
				.get("encoding")
				.and_then(Value::as_str)
				.unwrap_or("text");

			match encoding {
				"base64" => {
					use base64::prelude::*;
					let decoded = BASE64_STANDARD
						.decode(content)
						.map_err(|e| anyhow!("Base64 decode failed: {}", e))?;
					Ok(Bytes::from(decoded))
				}
				"hex" => {
					let decoded = hex::decode(content).map_err(|e| anyhow!("Hex decode failed: {}", e))?;
					Ok(Bytes::from(decoded))
				}
				"text" | "utf8" => Ok(Bytes::copy_from_slice(content.as_bytes())),
				_ => Err(anyhow!("Unknown encoding: {}", encoding)),
			}
		}
		_ => Err(anyhow!("Invalid body format. Expected String or Object.")),
	}
}
