//! Compile-time uniformity check for per-listener mTLS posture.
//!
//! mTLS is a listener-level setting (one `ServerConfig` per listener),
//! so every rule sharing a listener must declare the same
//! `client_auth` — *including* rules that omit the field. A mixed
//! population (some rules `Some(Require{...})`, others `None`) would
//! silently impose a posture the omitting rule's author never asked
//! for; the compile pipeline now rejects that mixture.

use std::path::PathBuf;

use serde_json::json;
use vane_core::compile::{RawRuleFile, compile};
use vane_core::error::Error;
use vane_core::fetch::{FetchKind, FetchOutputModes, FetchPhase};
use vane_core::metadata::{
	FetchMetadata, FetchMetadataProvider, MiddlewareMetadata, MiddlewareMetadataProvider,
};
use vane_core::middleware::MiddlewareKind;
use vane_core::preset::RuleEntry;

struct Providers;

fn validate_ok(_: &serde_json::Value) -> Result<(), Error> {
	Ok(())
}

impl MiddlewareMetadataProvider for Providers {
	fn get(&self, _: &str) -> Option<MiddlewareMetadata> {
		Some(MiddlewareMetadata {
			kind: MiddlewareKind::L7Request,
			stateless: true,
			needs_body: false,
			validate_args: validate_ok,
		})
	}
}

impl FetchMetadataProvider for Providers {
	fn get(&self, kind: FetchKind) -> Option<FetchMetadata> {
		Some(FetchMetadata {
			kind,
			phase: match kind {
				FetchKind::L4Forward => FetchPhase::L4,
				_ => FetchPhase::L7,
			},
			output_modes: FetchOutputModes { response: true, tunnel: false },
			validate_args: validate_ok,
		})
	}
}

fn rule_file(entries: Vec<RuleEntry>) -> RawRuleFile {
	RawRuleFile { path: PathBuf::from("rules/client_auth.json"), order: 0, rules: entries }
}

fn parse_entry(raw: serde_json::Value) -> RuleEntry {
	serde_json::from_value(raw).expect("parse rule entry")
}

fn rule_tls_no_client_auth(name: &str, host: &str) -> serde_json::Value {
	json!({
		"name": name,
		"listen": [":8443"],
		"match": { "tls.sni": { "equals": host } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": {
			"sni": host,
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": false,
		},
	})
}

fn rule_tls_client_auth_require(name: &str, host: &str) -> serde_json::Value {
	json!({
		"name": name,
		"listen": [":8443"],
		"match": { "tls.sni": { "equals": host } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": {
			"sni": host,
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": false,
			"client_auth": {
				"mode": "require",
				"trust_store": { "ca_paths": ["/tmp/ca.pem"] },
			},
		},
	})
}

#[test]
fn mixed_client_auth_none_and_require_rejected_with_posture_message() {
	let rules = vec![
		rule_tls_no_client_auth("a", "a.example.com"),
		rule_tls_client_auth_require("b", "b.example.com"),
	];
	let entries: Vec<RuleEntry> = rules.into_iter().map(parse_entry).collect();
	let err = compile(vec![rule_file(entries)], &Providers, &Providers)
		.expect_err("mixed client_auth must reject");
	let msg = err.to_string();
	assert!(msg.contains("client_auth"), "error names the field: {msg}");
	assert!(msg.contains("posture"), "error mentions posture: {msg}");
}

#[test]
fn all_rules_omitting_client_auth_compile_cleanly() {
	let entries = vec![
		parse_entry(rule_tls_no_client_auth("a", "a.example.com")),
		parse_entry(rule_tls_no_client_auth("b", "b.example.com")),
	];
	compile(vec![rule_file(entries)], &Providers, &Providers)
		.expect("uniform posture (all None) compiles");
}

#[test]
fn all_rules_declaring_same_require_compile_cleanly() {
	let entries = vec![
		parse_entry(rule_tls_client_auth_require("a", "a.example.com")),
		parse_entry(rule_tls_client_auth_require("b", "b.example.com")),
	];
	compile(vec![rule_file(entries)], &Providers, &Providers)
		.expect("uniform Require posture compiles");
}
