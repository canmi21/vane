/* src/transport/src/protocol/detect.rs */

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use guess::{DetectionStatus, Protocol};
use serde_json::Value;
use std::{any::Any, borrow::Cow};
use vane_engine::engine::interfaces::{
	GenericMiddleware, Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};

/// Core detection logic. Delegates to the `guess` crate for protocol matching.
fn detect(payload: &[u8], method: &str) -> bool {
	let protocol = match method {
		"http" => Protocol::Http,
		"tls" => Protocol::Tls,
		"dns" => Protocol::Dns,
		"quic" => Protocol::Quic,
		_ => return false,
	};
	matches!(protocol.probe(payload), DetectionStatus::Match)
}

/// A built-in Middleware plugin for basic L4 protocol detection.
pub struct ProtocolDetectPlugin;

impl Plugin for ProtocolDetectPlugin {
	fn name(&self) -> &'static str {
		"internal.protocol.detect"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef { name: "method".into(), required: true, param_type: ParamType::String },
			ParamDef { name: "payload".into(), required: true, param_type: ParamType::Bytes },
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

		Ok(MiddlewareOutput { branch: branch.into(), store: None })
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
		let mut valid_dns =
			vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
		// Append "example.com" (7example3com0) type A class IN
		valid_dns.extend_from_slice(&[
			0x07, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x03, 0x63, 0x6f, 0x6d, 0x00, 0x00, 0x01,
			0x00, 0x01,
		]);
		assert!(detect(&valid_dns, "dns"), "Valid DNS query should be detected");

		// 2. DNS Response (QR bit set)
		// Flags=0x8180 (QR=1, RD, RA) — guess correctly identifies DNS responses too.
		let mut response = valid_dns.clone();
		response[2] = 0x81;
		response[7] = 0x01; // ANCOUNT=1 for a valid response
		assert!(detect(&response, "dns"), "DNS response should be detected");

		// 3. Invalid Opcode (Opcode=3, undefined)
		// Flags byte: 00011001 (QR=0, Opcode=3, RD=1)
		let mut bad_opcode = valid_dns.clone();
		bad_opcode[2] = 0x19;
		assert!(!detect(&bad_opcode, "dns"), "Invalid Opcode should be rejected");

		// 4. Zero QDCOUNT
		let mut zero_questions = valid_dns.clone();
		zero_questions[4] = 0x00;
		zero_questions[5] = 0x00;
		assert!(!detect(&zero_questions, "dns"), "QDCOUNT=0 should be rejected");

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

		// QUIC v2 (RFC 9369: version 0x6b3343cf)
		let v2_bytes = 0x6b3343cfu32.to_be_bytes();
		let mut quic_v2 = vec![0xC0, v2_bytes[0], v2_bytes[1], v2_bytes[2], v2_bytes[3]];
		quic_v2.extend_from_slice(&[0x00; 20]);
		assert!(detect(&quic_v2, "quic"));

		// Short Header (0xxxxxxx) - Should be rejected by this strict heuristic
		let mut short_header = vec![0x40, 0xAB, 0xCD, 0xEF]; // 0100...
		short_header.extend_from_slice(&[0x00; 20]);
		assert!(!detect(&short_header, "quic"), "Short header rejected");

		// Wrong Version (arbitrary non-QUIC value)
		let mut bad_version = vec![0xC0, 0x00, 0x00, 0x00, 0x03];
		bad_version.extend_from_slice(&[0x00; 20]);
		assert!(!detect(&bad_version, "quic"), "Invalid version rejected");
	}
}
