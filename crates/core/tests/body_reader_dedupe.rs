//! Compile-time regression: when a rule legitimately exercises all
//! three independent body-collection mechanisms — a `match.http.body`
//! predicate, a `retry.buffering = "force"` retry policy on the
//! fetch, and a `needs_body` middleware in the chain — the lower
//! pass must dedupe the redundant `collect_body_before` flags so
//! `validate_unique_body_reader_per_path` accepts the rule. See
//! `crates/core/src/compile/lower.rs` § `dedupe_body_collect_per_path`.
//!
//! Before the dedupe pass landed, this combination compiled to a
//! path with three `Some(Request)` collectors and the validator
//! rejected the rule outright with "path through listener entry has
//! more than one collect_body_before=Some(Request)".

use std::path::PathBuf;

use serde_json::json;
use vane_core::compile::{RawRuleFile, compile};
use vane_core::error::Error;
use vane_core::fetch::{FetchKind, FetchOutputModes, FetchPhase};
use vane_core::ir::{BodySide, Node};
use vane_core::metadata::{
	FetchMetadata, FetchMetadataProvider, MiddlewareMetadata, MiddlewareMetadataProvider,
};
use vane_core::middleware::MiddlewareKind;
use vane_core::preset::RuleEntry;

/// Two-middleware provider: `body_reader` declares `needs_body = true`;
/// everything else is the inert default. Lets the test fixture trigger
/// `mark_first_body_reader_dfs` on demand by referencing `body_reader`
/// in the middleware chain.
struct Providers;

fn validate_ok(_: &serde_json::Value) -> Result<(), Error> {
	Ok(())
}

impl MiddlewareMetadataProvider for Providers {
	fn get(&self, name: &str) -> Option<MiddlewareMetadata> {
		let needs_body = name == "body_reader";
		Some(MiddlewareMetadata {
			kind: MiddlewareKind::L7Request,
			stateless: true,
			needs_body,
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
	RawRuleFile { path: PathBuf::from("rules/body_reader_dedupe.json"), order: 0, rules: entries }
}

fn parse_entry(raw: serde_json::Value) -> RuleEntry {
	serde_json::from_value(raw).expect("parse rule entry")
}

#[test]
fn body_predicate_needs_body_middleware_and_retry_force_all_compile_cleanly() {
	// All three body-collection mechanisms on one rule:
	// 1. `match.http.body.contains "ping"` → Check on HttpBody flags collect_body_before
	// 2. `body_reader` middleware in chain (needs_body=true) → mark_first_body_reader_dfs flags
	// 3. `retry.buffering = "force"` + `max_attempts = 3` → Fetch flags collect_body_before
	let entry = parse_entry(json!({
		"name": "api",
		"listen": [":7000"],
		"match": {
			"http.body": { "contains": "ping" },
		},
		"middleware_chain": [
			{ "use": "body_reader" },
		],
		"terminate": {
			"type": "http_proxy",
			"upstream": "127.0.0.1:8080",
			"retry": {
				"max_attempts": 3,
				"buffering": "force",
			},
		},
	}));
	let graph = compile(vec![rule_file(vec![entry])], &Providers, &Providers)
		.expect("rule combining all three body-collection mechanisms must compile");
	let request_flags: usize = graph
		.nodes
		.iter()
		.filter(|n| {
			matches!(
				n,
				Node::Check { collect_body_before: Some(BodySide::Request), .. }
					| Node::Middleware { collect_body_before: Some(BodySide::Request), .. }
					| Node::Fetch { collect_body_before: Some(BodySide::Request), .. },
			)
		})
		.count();
	assert_eq!(
		request_flags, 1,
		"dedupe must collapse the three independent collect_body_before \
		 sources into exactly one node carrying the flag on the request side; \
		 found {request_flags}",
	);
}

#[test]
fn body_predicate_alone_keeps_its_flag() {
	// Sanity: a rule with only the `match.http.body` mechanism still
	// emits exactly one collector — the Check node — and the dedupe
	// pass is a no-op.
	let entry = parse_entry(json!({
		"name": "api",
		"listen": [":7001"],
		"match": {
			"http.body": { "contains": "ping" },
		},
		"terminate": {
			"type": "http_proxy",
			"upstream": "127.0.0.1:8080",
		},
	}));
	let graph = compile(vec![rule_file(vec![entry])], &Providers, &Providers).expect("compile ok");
	let request_flags: usize = graph
		.nodes
		.iter()
		.filter(|n| {
			matches!(
				n,
				Node::Check { collect_body_before: Some(BodySide::Request), .. }
					| Node::Middleware { collect_body_before: Some(BodySide::Request), .. }
					| Node::Fetch { collect_body_before: Some(BodySide::Request), .. },
			)
		})
		.count();
	assert_eq!(request_flags, 1, "single mechanism rule keeps exactly one collector");
}

#[test]
fn needs_body_middleware_and_retry_force_dedupe_to_one_collector() {
	// Two mechanisms: needs-body middleware first, then retry-force fetch.
	// Mark passes set the flag on the Middleware; retry-force on the
	// Fetch. Dedupe must clear the Fetch flag since the Middleware
	// comes first on the linear path.
	let entry = parse_entry(json!({
		"name": "api",
		"listen": [":7002"],
		"middleware_chain": [
			{ "use": "body_reader" },
		],
		"terminate": {
			"type": "http_proxy",
			"upstream": "127.0.0.1:8080",
			"retry": {
				"max_attempts": 3,
				"buffering": "force",
			},
		},
	}));
	let graph = compile(vec![rule_file(vec![entry])], &Providers, &Providers).expect("compile ok");
	let request_flags: usize = graph
		.nodes
		.iter()
		.filter(|n| {
			matches!(
				n,
				Node::Check { collect_body_before: Some(BodySide::Request), .. }
					| Node::Middleware { collect_body_before: Some(BodySide::Request), .. }
					| Node::Fetch { collect_body_before: Some(BodySide::Request), .. },
			)
		})
		.count();
	assert_eq!(request_flags, 1, "two-mechanism rule must dedupe to one collector");
	// The surviving collector should be the Middleware (it comes
	// first on the linear path).
	let on_middleware = graph
		.nodes
		.iter()
		.any(|n| matches!(n, Node::Middleware { collect_body_before: Some(BodySide::Request), .. }));
	assert!(on_middleware, "dedupe keeps the earliest collector (the needs-body Middleware)");
}
