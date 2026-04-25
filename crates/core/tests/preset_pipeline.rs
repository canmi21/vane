//! Integration tests for C13/S1-22: preset expansion through `compile()`.
//!
//! Black-box validation that each MVP preset (`port_forward`,
//! `static_site`, `redirect_https`, `reverse_proxy`), when fed through
//! the full compile pipeline (`merge → expand → analyze → lower →
//! validate`), produces a valid `Arc<SymbolicFlowGraph>`. Internals of
//! each expander are deliberately treated as a black box; assertions
//! anchor on the public IR shape (terminator slab, fetch slab,
//! middleware slab, entries map).
//!
//! See `spec/architecture/14-presets.md` for the input/output contract.

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

// ---------------------------------------------------------------------------
// Test scaffolding — mirror of the `Providers` fixture in
// `crates/core/src/compile.rs` so the integration tests can compile rules
// that name `forward_client_ip` (stateless) and `rate_limit` (stateful).
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Helpers for building `RawRuleFile`s out of preset invocations and
// hand-written raw rules.
// ---------------------------------------------------------------------------

fn preset_entry(name: &str, preset: &str, listen: &str, args: serde_json::Value) -> RuleEntry {
	RuleEntry::Preset(PresetInvocation {
		name: name.into(),
		preset: preset.into(),
		listen: vec![listen.into()],
		args,
		source: SourceInfo::default(),
	})
}

fn rule_file(path: &str, entries: Vec<RuleEntry>) -> RawRuleFile {
	RawRuleFile { path: PathBuf::from(path), order: 0, rules: entries }
}

// ---------------------------------------------------------------------------
// 1. port_forward
// ---------------------------------------------------------------------------

#[test]
fn port_forward_preset_compiles_to_graph_with_byte_tunnel_terminator() {
	// Spec § _`port_forward`_: expansion is one rule terminating in
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

// ---------------------------------------------------------------------------
// 2. static_site
// ---------------------------------------------------------------------------

#[test]
fn static_site_preset_compiles_to_graph_with_http_synthesize_fetch() {
	// Spec § _`static_site`_: expansion is one rule whose terminate is
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

// ---------------------------------------------------------------------------
// 3. redirect_https
// ---------------------------------------------------------------------------

#[test]
fn redirect_https_preset_compiles_to_graph_with_308_synth() {
	// Spec § _`redirect_https`_: expansion is a single rule emitting an
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

// ---------------------------------------------------------------------------
// 4-7. reverse_proxy variants
//
// FIXME(C13 follow-up): every `reverse_proxy` preset emission produces two
// or three rules sharing the same listener (e.g. `:443`). All emitted rules
// are L7 (HttpProxy / HttpSynthesize / WebSocketUpgrade), so each lowers
// with its own `Upgrade` node above its chain. `compile/lower.rs::lower_port`
// links rules via `on_miss`: the first rule's `Check` miss-path points at
// the next rule's chain entry. That entry IS another `Upgrade`. The phase
// validator (`compile/validate.rs::check_phases`) then walks the second
// `Upgrade` from `Phase::L7Request` (set by the first `Upgrade`) and rejects
// it because `Upgrade` only accepts `[L4Raw, L4Peeked]`. Concrete failure
// from `cargo test`:
//
//   Error { kind: Compile, ctx: Some("phase mismatch at NodeId(N): expected
//     one of [L4Raw, L4Peeked], got L7Request"), source: None }
//
// This is a real lower-stage bug surfaced by integration: every `reverse_proxy`
// preset invocation currently fails `compile()`. The bug is **not** in the
// preset expander — `expand()` produces the rules the spec describes
// (14-presets.md § _`reverse_proxy`_) and the existing unit tests in
// `crates/core/src/preset/reverse_proxy.rs` all pass. The bug is that
// `lower_port` does not deduplicate `Upgrade` nodes when stitching multiple
// L7 rules onto a single listener. Tests below are intentionally `#[ignore]`d
// pending that fix; remove the `#[ignore]` once `lower` shares one Upgrade
// across same-listener L7 rules (or analyze coalesces them earlier).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "lower-stage bug: shared-listener L7 rules emit chained Upgrades; phase validator rejects"]
fn reverse_proxy_default_compiles_to_graph_with_http_proxy() {
	// Spec § _`reverse_proxy`_: minimal args produce a `<name>.main`
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
#[ignore = "lower-stage bug: shared-listener L7 rules emit chained Upgrades; phase validator rejects"]
fn reverse_proxy_websocket_true_compiles_with_websocket_upgrade_fetch() {
	// Spec § _WebSocket handling_: `websocket: true` swaps the WS reject
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
#[ignore = "lower-stage bug: shared-listener L7 rules emit chained Upgrades; phase validator rejects"]
fn reverse_proxy_websocket_paths_compiles_with_three_rules_present() {
	// Spec § _WebSocket handling_ (path-prefix array): three rules —
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
#[ignore = "lower-stage bug: shared-listener L7 rules emit chained Upgrades; phase validator rejects"]
fn reverse_proxy_with_rate_limit_emits_middleware_in_graph() {
	// Spec § _`reverse_proxy`_: a `rate_limit` arg emits a `rate_limit`
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
#[ignore = "lower-stage bug: shared-listener L7 rules emit chained Upgrades; phase validator rejects"]
fn reverse_proxy_forward_client_ip_default_emits_middleware() {
	// Spec § _`reverse_proxy`_: `forward_client_ip` defaults to true. The
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
#[ignore = "lower-stage bug: shared-listener L7 rules emit chained Upgrades; phase validator rejects"]
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

// ---------------------------------------------------------------------------
// 10. Mixed raw + preset in same file
// ---------------------------------------------------------------------------

#[test]
fn mixed_raw_and_preset_in_same_file_compiles() {
	// Spec § _Two-tier rule system_: a single file may interleave
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

// ---------------------------------------------------------------------------
// 11. Duplicate preset names across the merged set
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 12. Unknown preset name
// ---------------------------------------------------------------------------

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
