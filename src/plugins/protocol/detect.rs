/* src/plugins/protocol/detect.rs */

use crate::engine::interfaces::{
	GenericMiddleware, Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use std::{any::Any, borrow::Cow};

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
				name: "method".into(),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "payload".into(),
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

	fn as_generic_middleware(&self) -> Option<&dyn GenericMiddleware> {
		Some(self)
	}
}

#[async_trait]
impl GenericMiddleware for ProtocolDetectPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["true".into(), "false".into()]
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
			branch: branch.into(),
			store: None,
		})
	}
}

#[async_trait]
impl Middleware for ProtocolDetectPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		<Self as GenericMiddleware>::output(self)
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		<Self as GenericMiddleware>::execute(self, inputs).await
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_dns_detection() {
		// 1. Valid DNS Query
		// ID=1234, Flags=0x0100 (RD), QD=1, AN=0, NS=0, AR=0
		let mut valid_dns = vec![
			0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
		];
		// Append "example.com" (7example3com0) type A class IN
		valid_dns.extend_from_slice(&[
			0x07, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x03, 0x63, 0x6f, 0x6d, 0x00, 0x00, 0x01,
			0x00, 0x01,
		]);
		assert!(
			detect(&valid_dns, "dns"),
			"Valid DNS query should be detected"
		);

		// 2. DNS Response (QR bit set)
		// Flags=0x8180 (QR=1, RD, RA)
		let mut response = valid_dns.clone();
		response[2] = 0x81;
		assert!(!detect(&response, "dns"), "DNS response should be rejected");

		// 3. Invalid Opcode (Opcode=1, IQUERY - obsolete but testing the bitmask)
		// Flags=0x0900 (QR=0, Opcode=1, RD)
		let mut bad_opcode = valid_dns.clone();
		bad_opcode[2] = 0x09;
		assert!(
			!detect(&bad_opcode, "dns"),
			"Non-standard Opcode should be rejected"
		);

		// 4. Zero QDCOUNT
		let mut zero_questions = valid_dns.clone();
		zero_questions[4] = 0x00;
		zero_questions[5] = 0x00;
		assert!(
			!detect(&zero_questions, "dns"),
			"QDCOUNT=0 should be rejected"
		);

		// 5. Truncated Header
		assert!(!detect(&valid_dns[..10], "dns"), "Truncated header");
	}

	#[test]
	fn test_http_detection() {
		assert!(detect(b"GET / HTTP/1.1\r\n", "http"));
		assert!(detect(b"POST /api/v1/submit HTTP/1.1\r\n", "http"));
		assert!(detect(b"HEAD /index.html HTTP/1.1\r\n", "http"));
		assert!(!detect(b"HELLO WORLD", "http"));
		assert!(!detect(b"SSH-2.0", "http"));
	}

	#[test]
	fn test_tls_detection() {
		// TLS ClientHello (0x16, 0x03, 0x01)
		let tls_handshake = [0x16, 0x03, 0x01, 0x00, 0x50];
		assert!(detect(&tls_handshake, "tls"));

		// SSLv3 (0x16, 0x03, 0x00) - Vane only checks 0x16 0x03
		let sslv3 = [0x16, 0x03, 0x00, 0x00, 0x10];
		assert!(detect(&sslv3, "tls"));

		// Random junk
		assert!(!detect(&[0x00, 0x01, 0x02], "tls"));
	}

	#[test]
	fn test_quic_detection() {
		// QUIC v1 Initial Packet
		// Header Form: 1 (Long)
		// Fixed Bit: 1
		// Type: 00 (Initial)
		// Byte 0 = 11000000 = 0xC0
		// Version = 1
		let mut quic_initial = vec![0xC0, 0x00, 0x00, 0x00, 0x01];
		// Pad to > 20 bytes
		quic_initial.extend_from_slice(&[0x00; 20]);

		assert!(detect(&quic_initial, "quic"));

		// QUIC v2 (Version 2)
		let mut quic_v2 = vec![0xC0, 0x00, 0x00, 0x00, 0x02];
		quic_v2.extend_from_slice(&[0x00; 20]);
		assert!(detect(&quic_v2, "quic"));

		// Short Header (0xxxxxxx) - Should be rejected by this strict heuristic
		let mut short_header = vec![0x40, 0xAB, 0xCD, 0xEF]; // 0100...
		short_header.extend_from_slice(&[0x00; 20]);
		assert!(!detect(&short_header, "quic"), "Short header rejected");

		// Wrong Version
		let mut bad_version = vec![0xC0, 0x00, 0x00, 0x00, 0x00]; // Version negotiation / 0
		bad_version.extend_from_slice(&[0x00; 20]);
		assert!(!detect(&bad_version, "quic"), "Version 0 rejected");
	}
}
