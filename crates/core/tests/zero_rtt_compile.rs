//! Compile-time validation for the TLS 1.3 0-RTT (early data) feature.
//!
//! Drives the full `merge → expand → analyze → lower → validate`
//! pipeline through `compile()` so each error path here matches what
//! an operator would see from `vane compile --dry-run`. See
//! `spec/crates/engine-tls.md` § _TLS 1.3 0-RTT (early data)_
//! § _Configuration schema_ for the rule list.

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

#[allow(clippy::unnecessary_wraps)]
fn validate_ok(_: &serde_json::Value) -> Result<(), Error> {
	Ok(())
}

impl MiddlewareMetadataProvider for Providers {
	fn get(&self, _name: &str) -> Option<MiddlewareMetadata> {
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
			output_modes: match kind {
				FetchKind::L4Forward => FetchOutputModes { response: false, tunnel: true },
				FetchKind::WebSocketUpgrade => FetchOutputModes { response: true, tunnel: true },
				_ => FetchOutputModes { response: true, tunnel: false },
			},
			validate_args: validate_ok,
		})
	}
}

fn rule_file(entries: Vec<RuleEntry>) -> RawRuleFile {
	RawRuleFile { path: PathBuf::from("rules/zero_rtt.json"), order: 0, rules: entries }
}

fn parse_entry(raw: serde_json::Value) -> RuleEntry {
	serde_json::from_value(raw).expect("parse rule entry")
}

fn compile_one(raw: serde_json::Value) -> Result<(), Error> {
	let entry = parse_entry(raw);
	compile(vec![rule_file(vec![entry])], &Providers, &Providers).map(|_| ())
}

#[test]
fn missing_allow_zero_rtt_on_tls_l7_rule_errors() {
	// `tls.enable_zero_rtt` is present but the per-rule `allow_zero_rtt`
	// is absent — required by `spec/crates/engine-tls.md` § _TLS 1.3 0-RTT_ § _Compile-
	// time constraints_.
	let err = compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": false,
		},
	}))
	.expect_err("missing allow_zero_rtt on TLS-L7 rule must error");
	let msg = err.to_string();
	assert!(msg.contains("allow_zero_rtt"), "error names the field: {msg}");
	assert!(msg.contains("api"), "error names the rule: {msg}");
}

#[test]
fn allow_zero_rtt_on_plaintext_rule_errors() {
	// Rule on a plaintext listener (no `tls` block) with `allow_zero_rtt`
	// set is meaningless — the rule's listener never terminates TLS.
	let err = compile_one(json!({
		"name": "api",
		"listen": [":80"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
	}))
	.expect_err("allow_zero_rtt on plaintext rule must error");
	let msg = err.to_string();
	assert!(msg.contains("allow_zero_rtt"), "error names the field: {msg}");
	assert!(msg.contains("L7 rules"), "error explains the constraint: {msg}");
}

#[test]
fn allow_zero_rtt_on_l4_rule_errors() {
	// L4 forward rules cannot use `allow_zero_rtt` — there's no HTTP
	// layer to inspect a method on.
	let err = compile_one(json!({
		"name": "ssh",
		"listen": ["tcp:2222"],
		"terminate": { "type": "tcp_forward", "upstream": "10.0.0.5:22" },
		"allow_zero_rtt": false,
	}))
	.expect_err("allow_zero_rtt on L4 rule must error");
	let msg = err.to_string();
	assert!(msg.contains("allow_zero_rtt"), "error names the field: {msg}");
}

#[test]
fn allow_zero_rtt_true_on_listener_with_enable_false_errors() {
	// Listener-level `enable_zero_rtt: false` means rustls won't accept
	// early data; a rule that opts in is dead config.
	let err = compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"match": { "http.method": { "equals": "GET" } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": false,
		},
	}))
	.expect_err("allow_zero_rtt true with enable_zero_rtt false must error");
	let msg = err.to_string();
	assert!(msg.contains("allow_zero_rtt: true"), "error names the rule field: {msg}");
	assert!(msg.contains("enable_zero_rtt: false"), "error names the listener field: {msg}");
	assert!(msg.contains("api"), "error names the rule: {msg}");
}

#[test]
fn allow_zero_rtt_true_without_method_constraint_errors() {
	// No `match` block at all — the rule accepts every method,
	// including POST. The idempotent-method gate fails.
	let err = compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": true,
		},
	}))
	.expect_err("allow_zero_rtt true without method constraint must error");
	let msg = err.to_string();
	assert!(msg.contains("method constraint"), "error explains the constraint: {msg}");
	assert!(msg.contains("GET / HEAD / OPTIONS"), "error names the idempotent set: {msg}");
}

#[test]
fn allow_zero_rtt_true_with_post_in_any_of_errors() {
	// `any_of [http.method equals GET, http.method equals POST]` admits
	// POST, so the gate must fail.
	let err = compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"match": {
			"any_of": [
				{ "http.method": { "equals": "GET" } },
				{ "http.method": { "equals": "POST" } },
			],
		},
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": true,
		},
	}))
	.expect_err("allow_zero_rtt true with POST in any_of must error");
	let msg = err.to_string();
	assert!(msg.contains("method constraint"), "error explains the constraint: {msg}");
}

#[test]
fn allow_zero_rtt_true_with_get_only_predicate_compiles() {
	// Happy path: a single GET equality satisfies the gate.
	compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"match": { "http.method": { "equals": "GET" } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": true,
		},
	}))
	.expect("GET-only predicate satisfies the idempotent gate");
}

#[test]
fn allow_zero_rtt_true_with_any_of_idempotent_only_compiles() {
	// `any_of [GET, HEAD, OPTIONS]` — every alternative is idempotent.
	compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"match": {
			"any_of": [
				{ "http.method": { "equals": "GET" } },
				{ "http.method": { "equals": "HEAD" } },
				{ "http.method": { "equals": "OPTIONS" } },
			],
		},
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": true,
		},
	}))
	.expect("any_of of idempotent methods satisfies the gate");
}

#[test]
fn allow_zero_rtt_true_with_all_of_containing_get_method_compiles() {
	// `all_of [http.method equals GET, http.uri.path prefix /api]` —
	// the GET check narrows the allowed methods to {GET}; conjoined
	// with the path constraint, the rule still only matches GET.
	compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"match": {
			"all_of": [
				{ "http.method": { "equals": "GET" } },
				{ "http.uri.path": { "prefix": "/api" } },
			],
		},
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": true,
		},
	}))
	.expect("all_of with GET conjunct satisfies the gate");
}

#[test]
fn rules_disagree_on_enable_zero_rtt_listener_aggregation_errors() {
	// Two rules sharing one listener disagree on `tls.enable_zero_rtt`
	// — the listener has one `ServerConfig`, so the values must agree.
	let a = parse_entry(json!({
		"name": "api-a",
		"listen": [":443"],
		"match": { "tls.sni": { "equals": "a.example.com" } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": {
			"sni": "a.example.com",
			"cert_file": "/tmp/a.pem",
			"key_file": "/tmp/a.key",
			"enable_zero_rtt": true,
		},
	}));
	let b = parse_entry(json!({
		"name": "api-b",
		"listen": [":443"],
		"match": { "tls.sni": { "equals": "b.example.com" } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8081" },
		"allow_zero_rtt": false,
		"tls": {
			"sni": "b.example.com",
			"cert_file": "/tmp/b.pem",
			"key_file": "/tmp/b.key",
			"enable_zero_rtt": false,
		},
	}));
	let err = compile(vec![rule_file(vec![a, b])], &Providers, &Providers)
		.expect_err("conflicting enable_zero_rtt must error");
	let msg = err.to_string();
	assert!(msg.contains("enable_zero_rtt"), "error names the field: {msg}");
	assert!(
		msg.contains("agree") || msg.contains("disagree"),
		"error explains the aggregation requirement: {msg}",
	);
}

#[test]
fn allow_zero_rtt_true_with_method_in_idempotent_list_compiles() {
	// `http.method in ["GET", "HEAD"]` is the same shape as `equals`
	// against a single method; both should satisfy the gate.
	compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"match": { "http.method": { "in": ["GET", "HEAD"] } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": true,
		},
	}))
	.expect("method `in [GET, HEAD]` satisfies the gate");
}

#[test]
fn allow_zero_rtt_true_with_method_in_list_containing_post_errors() {
	let err = compile_one(json!({
		"name": "api",
		"listen": [":443"],
		"match": { "http.method": { "in": ["GET", "POST"] } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": true,
		},
	}))
	.expect_err("method `in [GET, POST]` admits POST and must error");
	let msg = err.to_string();
	assert!(msg.contains("method constraint"), "error explains the constraint: {msg}");
}

#[test]
fn enable_zero_rtt_aggregates_into_listener_spec() {
	// Compile a rule whose `tls.enable_zero_rtt: true` and verify the
	// listener-level aggregated value lands on `ListenerTlsSpec`.
	let entry = parse_entry(json!({
		"name": "api",
		"listen": [":443"],
		"match": { "http.method": { "equals": "GET" } },
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": true,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": true,
		},
	}));
	let graph = compile(vec![rule_file(vec![entry])], &Providers, &Providers).expect("compile");
	for spec in graph.meta.listener_tls.values() {
		assert!(spec.enable_zero_rtt, "listener-level enable_zero_rtt must reflect the rule's setting");
	}
}
