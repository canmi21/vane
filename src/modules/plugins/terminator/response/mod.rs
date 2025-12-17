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
use http::{HeaderValue, Response, StatusCode};
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
				name: "body".into(),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "content_type".into(),
				required: false,
				param_type: ParamType::String,
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

		// 1. Determine Status Code
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

		// 2. Prepare Headers (Native HeaderMap)
		// We use the container's native headers as the base.
		// No need to rebuild from KV.
		let headers = &mut container.response_headers;

		// 3. Handle Body & Content-Type Logic
		if let Some(static_body) = inputs.get("body").and_then(Value::as_str) {
			// Case A: Static Body from Input
			let bytes = Bytes::copy_from_slice(static_body.as_bytes());

			// Content-Type Strategy for Static Body
			if let Some(ct) = inputs.get("content_type").and_then(Value::as_str) {
				headers.insert(
					http::header::CONTENT_TYPE,
					HeaderValue::from_str(ct).unwrap_or(HeaderValue::from_static("text/plain")),
				);
			} else if !headers.contains_key(http::header::CONTENT_TYPE) {
				let mime = content_type::guess_mime(&bytes);
				headers.insert(
					http::header::CONTENT_TYPE,
					HeaderValue::from_str(mime).unwrap(),
				);
			}

			// VaneBody::Buffered expects Full<Bytes>
			let full_body = Full::new(bytes);
			container.response_body = PayloadState::Http(VaneBody::Buffered(full_body));
		} else {
			// Case B: Existing Body (from Upstream or empty)

			if let Some(ct) = inputs.get("content_type").and_then(Value::as_str) {
				headers.insert(
					http::header::CONTENT_TYPE,
					HeaderValue::from_str(ct).unwrap_or(HeaderValue::from_static("text/plain")),
				);
			} else if !headers.contains_key(http::header::CONTENT_TYPE) {
				// Sniff only if buffered
				if let PayloadState::Buffered(ref bytes) = container.response_body {
					let mime = content_type::guess_mime(bytes);
					headers.insert(
						http::header::CONTENT_TYPE,
						HeaderValue::from_str(mime).unwrap(),
					);
				}
			}
		}

		// 4. Construct Response (Headers Only)
		let mut response = Response::builder().status(status_code).body(()).unwrap();

		// Zero-Copy swap: Move the prepared headers into the Response object
		*response.headers_mut() = std::mem::take(headers);

		// 5. Signal Adapter
		if let Some(tx) = container.response_tx.take() {
			let _ = tx.send(response);
		} else {
			log(
				LogLevel::Warn,
				"⚠ SendResponse called but response channel is missing (already sent?).",
			);
		}

		Ok(TerminatorResult::Finished)
	}
}
