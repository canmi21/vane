/* src/plugins/l7/response/mod.rs */

pub mod content_type;

use crate::engine::interfaces::{
	L7Terminator, ParamDef, ParamType, Plugin, ResolvedInputs, TerminatorResult,
};
use crate::layers::l7::{
	container::{Container, PayloadState},
	http::wrapper::VaneBody,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use fancy_log::{LogLevel, log};
use http::{HeaderName, HeaderValue, Response, StatusCode};
use http_body_util::Full;
use serde_json::Value;
use std::any::Any;
use std::borrow::Cow;

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
				name: "body".into(),
				required: false,
				param_type: ParamType::Any,
			},
		]
	}

	fn supported_protocols(&self) -> Vec<Cow<'static, str>> {
		vec!["httpx".into()]
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

		// Check if this is a WebSocket upgrade (both handles present)
		if let (Some(client_upgrade), Some(upstream_upgrade)) = (
			container
				.http_data_mut()
				.and_then(|d| d.client_upgrade.take()),
			container
				.http_data_mut()
				.and_then(|d| d.upstream_upgrade.take()),
		) {
			log(
				LogLevel::Debug,
				"➜ Establishing WebSocket bidirectional tunnel...",
			);

			// Construct 101 Switching Protocols response
			let mut response = Response::builder()
				.status(StatusCode::SWITCHING_PROTOCOLS)
				.body(())
				.map_err(|e| anyhow!("Failed to build WebSocket 101 response: {e}"))?;

			// Use backend's response headers (contains Upgrade handshake headers)
			*response.headers_mut() = std::mem::take(&mut container.response_headers);

			// Send 101 response to client (signals httpx to send response)
			if let Some(tx) = container.response_tx.take() {
				if tx.send(response).is_err() {
					return Err(anyhow!("Failed to send WebSocket upgrade response"));
				}
			} else {
				return Err(anyhow!("Response channel missing for WebSocket upgrade"));
			}

			// Spawn tunnel in background to avoid deadlock
			// This allows httpx to complete serving the 101 response first,
			// which triggers the client_upgrade future to resolve
			tokio::spawn(async move {
				log(LogLevel::Debug, "⚙ Waiting for upgrade to complete...");

				// Wait for both upgrades (client has now received 101)
				let tunnel_result = tokio::try_join!(client_upgrade, upstream_upgrade);

				match tunnel_result {
					Ok((client_io, upstream_io)) => {
						// Wrap in TokioIo for AsyncRead + AsyncWrite
						let mut client_io = hyper_util::rt::TokioIo::new(client_io);
						let mut upstream_io = hyper_util::rt::TokioIo::new(upstream_io);

						log(
							LogLevel::Debug,
							"✓ WebSocket tunnel established, starting bidirectional copy",
						);

						match tokio::io::copy_bidirectional(&mut client_io, &mut upstream_io).await {
							Ok((client_to_upstream, upstream_to_client)) => {
								log(
									LogLevel::Debug,
									&format!(
										"✓ WebSocket tunnel closed gracefully. Client→Upstream: {client_to_upstream} bytes, Upstream→Client: {upstream_to_client} bytes"
									),
								);
							}
							Err(e) => {
								log(
									LogLevel::Warn,
									&format!("⚠ WebSocket tunnel I/O error: {e}"),
								);
							}
						}
					}
					Err(e) => {
						log(
							LogLevel::Error,
							&format!("✗ WebSocket upgrade failed: {e}"),
						);
					}
				}
			});

			// Return immediately (tunnel is running in background)
			return Ok(TerminatorResult::Finished);
		}

		// Normal HTTP response handling below

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
				let Ok(header_name) = HeaderName::from_bytes(k.as_bytes()) else { continue };

				match v {
					Value::String(s) => {
						if let Ok(val) = HeaderValue::from_str(s) {
							headers.insert(header_name, val);
						}
					}
					Value::Array(arr) => {
						for item in arr {
							if let Some(s) = item.as_str()
								&& let Ok(val) = HeaderValue::from_str(s) {
									headers.append(header_name.clone(), val);
								}
						}
					}
					_ => {}
				}
			}
		}

		// 3. Handle Body (Takeover vs Inherit vs KV)
		if let Some(body_input) = inputs.get("body") {
			// Takeover Mode: Overwrite body from config input
			let body_bytes = parse_body_input(body_input)?;

			// Auto-Guess Content-Type for static body if missing
			if !headers.contains_key(http::header::CONTENT_TYPE) {
				let mime = content_type::guess_mime(&body_bytes);
				headers.insert(
					http::header::CONTENT_TYPE,
					HeaderValue::from_str(mime).map_err(|e| anyhow!("Invalid mime type: {e}"))?,
				);
			}

			let full_body = Full::new(body_bytes);
			container.response_body = PayloadState::Http(VaneBody::Buffered(full_body));
		} else if let Some(body_str) = container.kv.get("res.body") {
			// Inherit from KV (for responses set by middleware like FetchUpstream)
			let body_bytes = Bytes::copy_from_slice(body_str.as_bytes());

			if !headers.contains_key(http::header::CONTENT_TYPE) {
				headers.insert(
					http::header::CONTENT_TYPE,
					HeaderValue::from_static("text/plain; charset=utf-8"),
				);
			}

			let full_body = Full::new(body_bytes);
			container.response_body = PayloadState::Http(VaneBody::Buffered(full_body));
		}
		// Keep existing response_body (from FetchUpstream)

		// 4. Construct Final Response
		let mut response = Response::builder()
			.status(status_code)
			.body(())
			.map_err(|e| anyhow!("Failed to build response: {e}"))?;
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
						.map_err(|e| anyhow!("Base64 decode failed: {e}"))?;
					Ok(Bytes::from(decoded))
				}
				"hex" => {
					let decoded = hex::decode(content).map_err(|e| anyhow!("Hex decode failed: {e}"))?;
					Ok(Bytes::from(decoded))
				}
				"text" | "utf8" => Ok(Bytes::copy_from_slice(content.as_bytes())),
				_ => Err(anyhow!("Unknown encoding: {encoding}")),
			}
		}
		_ => Err(anyhow!("Invalid body format. Expected String or Object.")),
	}
}
