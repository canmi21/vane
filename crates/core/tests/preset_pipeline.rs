//! Integration tests for preset expansion through `compile()`.
//!
//! Black-box validation that each MVP preset (`port_forward`,
//! `static_site`, `redirect_https`, `reverse_proxy`), when fed through
//! the full compile pipeline (`merge → expand → analyze → lower →
//! validate`), produces a valid `Arc<SymbolicFlowGraph>`. Internals of
//! each expander are deliberately treated as a black box; assertions
//! anchor on the public IR shape (terminator slab, fetch slab,
//! middleware slab, entries map).
//!
//! See `spec/crates/core.md` for the input/output contract.

use std::path::PathBuf;

use serde_json::json;
use vane_core::compile::{RawRuleFile, compile};
use vane_core::error::Error;
use vane_core::fetch::{FetchKind, FetchOutputModes, FetchPhase, Terminator};
use vane_core::metadata::{
	FetchMetadata, FetchMetadataProvider, MiddlewareMetadata, MiddlewareMetadataProvider,
};
use vane_core::middleware::MiddlewareKind;
use vane_core::preset::{PresetInvocation, RuleEntry};
use vane_core::rule::SourceInfo;

// Test scaffolding — mirror of the `Providers` fixture in
// `crates/core/src/compile.rs` so the integration tests can compile rules
// that name `forward_client_ip` (stateless) and `rate_limit` (stateful).

struct Providers;

#[allow(clippy::unnecessary_wraps)]
fn validate_ok(_: &serde_json::Value) -> Result<(), Error> {
	Ok(())
}

impl MiddlewareMetadataProvider for Providers {
	fn get(&self, name: &str) -> Option<MiddlewareMetadata> {
		match name {
			"forward_client_ip" => Some(MiddlewareMetadata {
				kind: MiddlewareKind::L7Request,
				stateless: true,
				needs_body: false,
				validate_args: validate_ok,
			}),
			"rate_limit" => Some(MiddlewareMetadata {
				kind: MiddlewareKind::L7Request,
				stateless: false,
				needs_body: false,
				validate_args: validate_ok,
			}),
			_ => None,
		}
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

// Helpers for building `RawRuleFile`s out of preset invocations and
// hand-written raw rules.

fn preset_entry(name: &str, preset: &str, listen: &str, args: serde_json::Value) -> RuleEntry {
	RuleEntry::Preset(PresetInvocation {
		name: name.into(),
		preset: preset.into(),
		listen: vec![listen.into()],
		args,
		tls: None,
		source: SourceInfo::default(),
	})
}

fn rule_file(path: &str, entries: Vec<RuleEntry>) -> RawRuleFile {
	RawRuleFile { path: PathBuf::from(path), order: 0, rules: entries }
}

#[test]
fn port_forward_preset_compiles_to_graph_with_byte_tunnel_terminator() {
	// `spec/crates/core.md` § _Compile pipeline_: expansion is one rule terminating in
	// `L4Forward`. Lowering an `L4Forward` fetch yields a
	// `Terminator::ByteTunnel` (see `compile.rs` test
	// `terminator_variant_derives_from_fetch_kind`).
	let entry = preset_entry("ssh", "port_forward", ":2222", json!({ "upstream": "10.0.0.5:22" }));
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("port_forward compiles");
	assert!(
		graph.terminators.iter().any(|t| matches!(t, Terminator::ByteTunnel)),
		"expected ByteTunnel terminator in {:?}",
		graph.terminators,
	);
}

#[test]
fn static_site_preset_compiles_to_graph_with_http_synthesize_fetch() {
	// `spec/crates/core.md` § _Compile pipeline_: expansion is one rule whose terminate is
	// `HttpSynthesize`.
	let entry = preset_entry(
		"hello",
		"static_site",
		":8443",
		json!({
			"status": 200,
			"headers": { "content-type": "text/plain" },
			"body": "Hello, world!",
		}),
	);
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("static_site compiles");
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpSynthesize),
		"expected HttpSynthesize in fetch slab",
	);
}

#[test]
fn redirect_https_preset_compiles_to_graph_with_308_synth() {
	// `spec/crates/core.md` § _Compile pipeline_: expansion is a single rule emitting an
	// `HttpSynthesize` 308 with `Location: https://${host}${uri}`. The
	// integration assertion only checks the fetch kind — body-shape
	// assertions belong inside the expander's own unit tests.
	let entry = preset_entry("rdr", "redirect_https", ":80", serde_json::Value::Null);
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("redirect_https compiles");
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpSynthesize),
		"expected HttpSynthesize in fetch slab",
	);
}

#[test]
fn reverse_proxy_default_compiles_to_graph_with_http_proxy() {
	// `spec/crates/core.md` § _Compile pipeline_: minimal args produce a `<name>.main`
	// (`HttpProxy`) and a `<name>.ws` reject (`HttpSynthesize` 400) when
	// `websocket` defaults to `false`.
	let entry = preset_entry("api", "reverse_proxy", ":443", json!({ "upstream": "127.0.0.1:8080" }));
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("reverse_proxy default compiles");
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpProxy),
		"expected HttpProxy fetch for the main route",
	);
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpSynthesize),
		"expected HttpSynthesize fetch for the WS reject",
	);
}

#[test]
fn reverse_proxy_websocket_true_compiles_with_websocket_upgrade_fetch() {
	// `spec/crates/engine.md` § _Concrete fetches_: `websocket: true` swaps the WS reject
	// for a `WebSocketUpgrade` fetch routed at upgrade-bearing requests.
	let entry = preset_entry(
		"api",
		"reverse_proxy",
		":443",
		json!({ "upstream": "127.0.0.1:8080", "websocket": true }),
	);
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("reverse_proxy websocket=true compiles");
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::WebSocketUpgrade),
		"expected WebSocketUpgrade fetch when websocket: true",
	);
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpProxy),
		"main HttpProxy route still present",
	);
}

#[test]
fn reverse_proxy_websocket_paths_compiles_with_three_rules_present() {
	// `spec/crates/engine.md` § _Concrete fetches_ (path-prefix array): three rules —
	// `<name>.ws-allow` (`WebSocketUpgrade`), `<name>.ws-deny`
	// (`HttpSynthesize` 400), `<name>.main` (`HttpProxy`). All three
	// fetch kinds must appear in the slab.
	let entry = preset_entry(
		"api",
		"reverse_proxy",
		":443",
		json!({ "upstream": "127.0.0.1:8080", "websocket": ["/ws", "/api/stream"] }),
	);
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("reverse_proxy websocket=[paths] compiles");
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::WebSocketUpgrade),
		"WebSocketUpgrade fetch (allow rule) must be present",
	);
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpSynthesize),
		"HttpSynthesize fetch (deny rule) must be present",
	);
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpProxy),
		"HttpProxy fetch (main rule) must be present",
	);
}

#[test]
fn reverse_proxy_with_rate_limit_emits_middleware_in_graph() {
	// `spec/crates/core.md` § _Compile pipeline_: a `rate_limit` arg emits a `rate_limit`
	// middleware in the main rule's chain. The `Providers` fixture
	// declares it stateful, so each call site gets its own slab entry.
	let entry = preset_entry(
		"api",
		"reverse_proxy",
		":443",
		json!({
			"upstream": "127.0.0.1:8080",
			"rate_limit": { "rate": 100, "burst": 200, "window": "1s" },
		}),
	);
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("reverse_proxy with rate_limit compiles");
	assert!(
		graph.middlewares.iter().any(|m| m.name.as_ref() == "rate_limit"),
		"expected rate_limit in middleware slab: {:?}",
		graph.middlewares.iter().map(|m| m.name.as_ref()).collect::<Vec<_>>(),
	);
}

#[test]
fn reverse_proxy_forward_client_ip_default_emits_middleware() {
	// `spec/crates/core.md` § _Compile pipeline_: `forward_client_ip` defaults to true. The
	// preset must emit it without an explicit arg.
	let entry = preset_entry("api", "reverse_proxy", ":443", json!({ "upstream": "127.0.0.1:8080" }));
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("reverse_proxy default compiles");
	assert!(
		graph.middlewares.iter().any(|m| m.name.as_ref() == "forward_client_ip"),
		"expected forward_client_ip in middleware slab by default: {:?}",
		graph.middlewares.iter().map(|m| m.name.as_ref()).collect::<Vec<_>>(),
	);
}

#[test]
fn reverse_proxy_forward_client_ip_false_no_middleware() {
	// Opt-out: `forward_client_ip: false` must remove the middleware.
	// The rest of the graph still compiles.
	let entry = preset_entry(
		"api",
		"reverse_proxy",
		":443",
		json!({ "upstream": "127.0.0.1:8080", "forward_client_ip": false }),
	);
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("reverse_proxy forward_client_ip=false compiles");
	assert!(
		!graph.middlewares.iter().any(|m| m.name.as_ref() == "forward_client_ip"),
		"forward_client_ip must be absent when explicitly disabled: {:?}",
		graph.middlewares.iter().map(|m| m.name.as_ref()).collect::<Vec<_>>(),
	);
}

#[test]
fn mixed_raw_and_preset_in_same_file_compiles() {
	// `spec/crates/core.md` § _Rate limit (L2)_: a single file may interleave
	// `RuleEntry::Raw` and `RuleEntry::Preset`. Both must reach the
	// graph, identified by terminator shape.
	let raw_entry: RuleEntry = serde_json::from_value(json!({
		"name": "raw_rule",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:9000" },
	}))
	.expect("parse raw rule");
	let preset = preset_entry("ssh", "port_forward", ":2222", json!({ "upstream": "10.0.0.5:22" }));
	let graph = compile(vec![rule_file("a.json", vec![raw_entry, preset])], &Providers, &Providers)
		.expect("mixed raw+preset compiles");
	// Raw HttpProxy → WriteHttpResponse, preset L4Forward → ByteTunnel.
	assert!(
		graph.terminators.iter().any(|t| matches!(t, Terminator::WriteHttpResponse)),
		"raw HttpProxy contributes WriteHttpResponse",
	);
	assert!(
		graph.terminators.iter().any(|t| matches!(t, Terminator::ByteTunnel)),
		"preset L4Forward contributes ByteTunnel",
	);
}

#[test]
fn two_reverse_proxy_presets_with_same_name_fail_at_expand_with_dup_error() {
	// `expand` runs the post-expansion duplicate-name check (see
	// `compile/expand.rs` doc-comment). Two `reverse_proxy` invocations
	// both named `"api"` collide on `<name>.main`.
	let a = preset_entry("api", "reverse_proxy", ":443", json!({ "upstream": "u1:1" }));
	let b = preset_entry("api", "reverse_proxy", ":8443", json!({ "upstream": "u2:2" }));
	let err = compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers)
		.expect_err("duplicate post-expansion name must fail");
	let msg = err.to_string();
	assert!(msg.contains("duplicate"), "error mentions duplicate: {msg}");
	assert!(msg.contains("api"), "error names the offending base name: {msg}");
}

#[test]
fn unknown_preset_name_fails_compile_with_pointed_error() {
	// Dispatcher rejects unknown names with a pointed message naming the
	// offending preset (see `preset/mod.rs::expand_invocation`).
	let entry = preset_entry("x", "no_such", ":443", json!({}));
	let err = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect_err("unknown preset must fail");
	let msg = err.to_string();
	assert!(msg.contains("no_such"), "error names the unknown preset: {msg}");
}

fn tls_preset_entry(
	name: &str,
	preset: &str,
	listen: &str,
	args: serde_json::Value,
	tls: Option<vane_core::rule::TlsConfig>,
) -> RuleEntry {
	RuleEntry::Preset(PresetInvocation {
		name: name.into(),
		preset: preset.into(),
		listen: vec![listen.into()],
		args,
		tls,
		source: SourceInfo::default(),
	})
}

#[test]
fn reverse_proxy_preset_propagates_tls_default_into_pool() {
	// The reverse_proxy expander emits up to three rules (.main / .ws-allow
	// / .ws-deny). Each carries the same `tls` block; the lower-stage
	// resolver hash-cons-dedups the identical entries into one pool slot.
	let tls = vane_core::rule::TlsConfig {
		sni: None,
		cert_file: Some("/tmp/cert.pem".into()),
		key_file: Some("/tmp/key.pem".into()),
		managed: None,
		enable_zero_rtt: false,
		client_auth: None,
		ocsp_path: None,
		ocsp_fetch: false,
	};
	let entry = tls_preset_entry(
		"api",
		"reverse_proxy",
		":443",
		json!({ "upstream": "127.0.0.1:8080", "websocket": ["/ws"] }),
		Some(tls.clone()),
	);
	let graph = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect("reverse_proxy with tls compiles");
	// `:443` shorthand expands to v4 + v6.
	assert_eq!(graph.meta.listener_tls.len(), 2);
	for spec in graph.meta.listener_tls.values() {
		assert_eq!(spec.default.as_ref(), Some(&tls));
		assert!(spec.sni_certs.is_empty());
	}
}

#[test]
fn raw_rule_with_tls_aggregates_into_listener_pool() {
	let raw_entry: RuleEntry = serde_json::from_value(json!({
		"name": "api",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": {
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"enable_zero_rtt": false,
		},
	}))
	.expect("parse raw rule with tls");
	let graph = compile(vec![rule_file("a.json", vec![raw_entry])], &Providers, &Providers)
		.expect("raw rule with tls compiles");
	let expected = vane_core::rule::TlsConfig {
		sni: None,
		cert_file: Some("/tmp/cert.pem".into()),
		key_file: Some("/tmp/key.pem".into()),
		managed: None,
		enable_zero_rtt: false,
		client_auth: None,
		ocsp_path: None,
		ocsp_fetch: false,
	};
	assert_eq!(graph.meta.listener_tls.len(), 2);
	for spec in graph.meta.listener_tls.values() {
		assert_eq!(spec.default.as_ref(), Some(&expected));
	}
}

#[test]
fn lower_aggregates_two_rules_same_port_distinct_sni_into_pool() {
	// Two rules on `:443` with distinct `sni` values — the lower stage
	// must keep both certs in the pool keyed by their SNI.
	let api: RuleEntry = serde_json::from_value(json!({
		"name": "api",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": { "sni": "api.example.com", "cert_file": "/tmp/api.pem", "key_file": "/tmp/api.key", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let admin: RuleEntry = serde_json::from_value(json!({
		"name": "admin",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8081" },
		"allow_zero_rtt": false,
		"tls": { "sni": "admin.example.com", "cert_file": "/tmp/admin.pem", "key_file": "/tmp/admin.key", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let graph = compile(vec![rule_file("a.json", vec![api, admin])], &Providers, &Providers)
		.expect("two distinct-sni rules compile");
	for spec in graph.meta.listener_tls.values() {
		assert!(spec.default.is_none(), "no rule supplied a sni-less default");
		assert_eq!(spec.sni_certs.len(), 2, "both SNIs land in the pool");
		assert!(spec.sni_certs.contains_key("api.example.com"));
		assert!(spec.sni_certs.contains_key("admin.example.com"));
	}
}

#[test]
fn lower_lowercases_sni_keys_in_pool() {
	// SNI hostnames are normalised to ASCII-lowercase per
	// spec/crates/engine-tls.md § _SNI peek (L4, no decrypt)_.
	let entry: RuleEntry = serde_json::from_value(json!({
		"name": "api",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": { "sni": "API.Example.COM", "cert_file": "/tmp/c.pem", "key_file": "/tmp/k.pem", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let graph =
		compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers).expect("compile");
	for spec in graph.meta.listener_tls.values() {
		assert!(spec.sni_certs.contains_key("api.example.com"));
		assert!(!spec.sni_certs.contains_key("API.Example.COM"));
	}
}

#[test]
fn lower_dedups_two_rules_same_port_identical_tls() {
	// Two rules pointing at the same `(sni, cert_file, key_file)` triple
	// should hash-cons into one pool entry, not two — and certainly
	// shouldn't error.
	let a: RuleEntry = serde_json::from_value(json!({
		"name": "api1",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": { "sni": "api.example.com", "cert_file": "/tmp/api.pem", "key_file": "/tmp/api.key", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let b: RuleEntry = serde_json::from_value(json!({
		"name": "api2",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8081" },
		"allow_zero_rtt": false,
		"tls": { "sni": "api.example.com", "cert_file": "/tmp/api.pem", "key_file": "/tmp/api.key", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let graph = compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers)
		.expect("identical TLS triples must dedup, not error");
	for spec in graph.meta.listener_tls.values() {
		assert_eq!(spec.sni_certs.len(), 1);
	}
}

#[test]
fn lower_rejects_two_rules_same_port_same_sni_different_certs() {
	let a: RuleEntry = serde_json::from_value(json!({
		"name": "api1",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": { "sni": "api.example.com", "cert_file": "/tmp/a.pem", "key_file": "/tmp/a.key", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let b: RuleEntry = serde_json::from_value(json!({
		"name": "api2",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8081" },
		"allow_zero_rtt": false,
		"tls": { "sni": "api.example.com", "cert_file": "/tmp/b.pem", "key_file": "/tmp/b.key", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let err = compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers)
		.expect_err("same SNI different certs must fail");
	let msg = err.to_string();
	assert!(msg.contains("api.example.com"), "error names the SNI: {msg}");
	assert!(
		msg.contains("/tmp/a.pem") && msg.contains("/tmp/b.pem"),
		"error names both cert paths: {msg}"
	);
}

#[test]
fn lower_rejects_two_rules_same_port_both_sniless_different_certs() {
	// A listener has at most one default (sni-less) cert.
	let a: RuleEntry = serde_json::from_value(json!({
		"name": "api1",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": { "cert_file": "/tmp/a.pem", "key_file": "/tmp/a.key", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let b: RuleEntry = serde_json::from_value(json!({
		"name": "api2",
		"listen": [":443"],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8081" },
		"allow_zero_rtt": false,
		"tls": { "cert_file": "/tmp/b.pem", "key_file": "/tmp/b.key", "enable_zero_rtt": false },
	}))
	.expect("parse");
	let err = compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers)
		.expect_err("two distinct sni-less certs must fail");
	let msg = err.to_string();
	assert!(msg.contains("more than one default"), "error mentions multiple defaults: {msg}");
}

#[test]
fn l4_listener_with_tls_block_is_rejected() {
	// `port_forward` produces an L4Forward rule; carrying `tls` on a
	// pure-byte-tunnel listener makes no sense (vane would terminate TLS
	// then re-emit plaintext to the upstream).
	let tls = vane_core::rule::TlsConfig {
		sni: None,
		cert_file: Some("/tmp/cert.pem".into()),
		key_file: Some("/tmp/key.pem".into()),
		managed: None,
		enable_zero_rtt: false,
		client_auth: None,
		ocsp_path: None,
		ocsp_fetch: false,
	};
	let entry = tls_preset_entry(
		"ssh",
		"port_forward",
		":2222",
		json!({ "upstream": "10.0.0.5:22" }),
		Some(tls),
	);
	let err = compile(vec![rule_file("a.json", vec![entry])], &Providers, &Providers)
		.expect_err("L4 listener with TLS must fail");
	let msg = err.to_string();
	assert!(msg.contains("TLS termination is L7-only"), "error explains the L4+TLS shape: {msg}");
}
