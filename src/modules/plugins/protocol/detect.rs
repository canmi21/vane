/* src/modules/plugins/protocol/detect.rs */

use crate::modules::plugins::model::{
	Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use std::any::Any;

/// Core detection logic. This is a pure, stateless function.
fn detect(payload: &[u8], method: &str) -> bool {
	if payload.is_empty() {
		return false;
	}
	match method {
		"http" => {
			payload.starts_with(b"GET ")
				|| payload.starts_with(b"POST ")
				|| payload.starts_with(b"PUT ")
				|| payload.starts_with(b"DELETE ")
				|| payload.starts_with(b"HEAD ")
				|| payload.starts_with(b"OPTIONS ")
				|| payload.starts_with(b"PATCH ")
		}
		"tls" => payload.starts_with(&[0x16, 0x03]) && payload.len() > 3,
		"dns" => {
			// Strict DNS Query Heuristic
			// Header: [ID: 2] [Flags: 2] [QDCOUNT: 2] ...
			if payload.len() < 12 {
				return false;
			}

			// Flags (Bytes 2-3)
			// Byte 2: [QR(1)] [Opcode(4)] [AA(1)] [TC(1)] [RD(1)]
			// QR must be 0 (Query)
			// Opcode should usually be 0 (Standard Query) for common traffic
			let flag_byte_1 = payload[2];

			// Check QR bit (0x80) is 0
			// Check Opcode bits (0x78) are 0
			if (flag_byte_1 & 0xF8) != 0 {
				return false;
			}

			// QDCOUNT (Bytes 4-5) must be > 0. A query with 0 questions is invalid.
			let qdcount = u16::from_be_bytes([payload[4], payload[5]]);
			qdcount > 0
		}
		"quic" => {
			// Strict QUIC v1 Initial Packet Heuristic (RFC 9000)

			// 1. Length Check
			// Client Initial packets are almost always padded to 1200 bytes.
			// However, to be safe against non-compliant or specialized clients,
			// we check for a minimal viable header size (Header + Version + CIDs).
			if payload.len() < 20 {
				return false;
			}

			// 2. Header Form Check (Byte 0)
			// Must be Long Header (0x80) AND Fixed Bit (0x40)
			// Pattern: 11xxxxxx (0xC0 mask)
			if (payload[0] & 0xC0) != 0xC0 {
				return false;
			}

			// 3. Version Check (Bytes 1-4)
			// This is the strongest check to prevent collision with random DNS IDs.
			// DNS ID (random) + Flags (0x0100 typically) is extremely unlikely to match
			// QUIC version 0x00000001.
			let version = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);

			// Support v1 (1) and v2 (2)
			version == 1 || version == 2
		}
		_ => false,
	}
}

/// A built-in Middleware plugin for basic L4 protocol detection.
pub struct ProtocolDetectPlugin;

impl Plugin for ProtocolDetectPlugin {
	fn name(&self) -> &'static str {
		"internal.protocol.detect"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "method",
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "payload",
				required: true,
				param_type: ParamType::Bytes,
			},
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		Some(self)
	}
}

#[async_trait]
impl Middleware for ProtocolDetectPlugin {
	fn output(&self) -> Vec<&'static str> {
		vec!["true", "false"]
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		let method = inputs
			.get("method")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'method' is missing or not a string"))?;

		let payload_hex = inputs
			.get("payload")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Resolved input 'payload' is missing or not a string"))?;
		let payload = hex::decode(payload_hex)?;

		let result = detect(&payload, method);
		let branch = if result { "true" } else { "false" };

		Ok(MiddlewareOutput {
			branch,
			write_to_kv: None,
		})
	}
}
