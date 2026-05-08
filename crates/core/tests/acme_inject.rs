//! End-to-end tests for the lower pass's ACME HTTP-01 inject step
//! (`spec/crates/engine-acme.md` § _Challenge: HTTP-01_).
//!
//! Fixtures drive the full `compile(merge → expand → analyze →
//! lower → validate)` facade so the assertions land on the same
//! flow-graph shape the daemon's link stage will receive.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use vane_core::compile::{RawRuleFile, compile};
use vane_core::error::Error;
use vane_core::fetch::{FetchKind, FetchOutputModes, FetchPhase, Terminator};
use vane_core::ir::{Node, SymbolicFlowGraph};
use vane_core::metadata::{
	FetchMetadata, FetchMetadataProvider, MiddlewareMetadata, MiddlewareMetadataProvider,
};

struct Providers;

#[allow(clippy::unnecessary_wraps)]
fn validate_ok(_: &Value) -> Result<(), Error> {
	Ok(())
}

impl MiddlewareMetadataProvider for Providers {
	fn get(&self, _name: &str) -> Option<MiddlewareMetadata> {
		None
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

fn rule_file(rules: Vec<Value>) -> RawRuleFile {
	RawRuleFile {
		path: PathBuf::from("test.json"),
		order: 0,
		rules: rules.into_iter().map(|r| serde_json::from_value(r).expect("parse rule")).collect(),
	}
}

fn plain_http_rule(name: &str, listen: &str) -> Value {
	json!({
		"name": name,
		"listen": [listen],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
	})
}

fn managed_https_rule(name: &str, listen: &str, sni: &str) -> Value {
	json!({
		"name": name,
		"listen": [listen],
		"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		"allow_zero_rtt": false,
		"tls": {
			"sni": sni,
			"managed": {
				"directory_url": "https://acme-staging-v02.api.letsencrypt.org/directory",
				"contact": ["mailto:ops@example.com"],
				"agree_tos": true,
				"challenge": "http-01",
				"key_type": "ecdsa-p256",
				"renew_before": "30d",
				"san": [sni],
			},
			"enable_zero_rtt": false,
		},
	})
}

/// Look up a listener's entry node by addr port; works regardless
/// of whether the lower pass has rewritten the entry to inject the
/// ACME route.
fn entry_for_port(graph: &Arc<SymbolicFlowGraph>, port: u16) -> &Node {
	let entry = graph
		.entries
		.iter()
		.find(|(addr, _)| addr.port() == port)
		.map(|(_, id)| *id)
		.expect("listener at port");
	&graph.nodes[entry.get() as usize]
}

/// Walk through an Upgrade if the entry is one — gives the L7
/// node the inject pass actually rewrites. The inject pass keeps
/// `Upgrade` as the listener entry (phase: `L4Raw`) and rewires
/// `Upgrade.next` to the synthetic Check (phase: `L7Request`); the
/// public assertions usually want to inspect the Check, not the
/// Upgrade.
fn post_upgrade_node<'a>(graph: &'a SymbolicFlowGraph, entry: &'a Node) -> &'a Node {
	match entry {
		Node::Upgrade { next } => &graph.nodes[next.get() as usize],
		other => other,
	}
}

#[test]
fn inject_no_op_when_no_managed_certs() {
	// A pure plaintext :80 listener with no managed certs anywhere
	// in the config: the inject pass must not rewrite anything in
	// the listener subgraph, and no annotations should be emitted.
	let entry = plain_http_rule("plain", ":80");
	let graph = compile(vec![rule_file(vec![entry])], &Providers, &Providers)
		.expect("compile plaintext-only :80");
	let node = post_upgrade_node(&graph, entry_for_port(&graph, 80));
	if let Node::Check { predicate, .. } = node {
		let dbg = format!("{:?}", graph.predicates[predicate.get() as usize]);
		assert!(!dbg.contains("acme-challenge"), "plain :80 listener must not have an ACME-path Check");
	}
	assert!(graph.meta.annotations.is_empty(), "no annotations expected on plaintext-only config");
}

#[test]
fn inject_rewrites_post_upgrade_on_plaintext_port_80_when_managed_http01_present() {
	// Two listeners: a managed :443 cert + a plaintext :80 redirect.
	// Per spec § _HTTP-01_: the listener entry stays an Upgrade
	// (phase: L4Raw); the inject pass rewires `Upgrade.next` to a
	// new Check whose predicate inspects `http.uri.path` (a
	// post-upgrade L7 field).
	let plain80 = plain_http_rule("plain", ":80");
	let managed = managed_https_rule("api", ":443", "api.example.com");
	let graph = compile(vec![rule_file(vec![plain80, managed])], &Providers, &Providers)
		.expect("compile mixed config");
	let node = post_upgrade_node(&graph, entry_for_port(&graph, 80));
	let pred_id = match node {
		Node::Check { predicate, .. } => *predicate,
		other => panic!("expected Check after Upgrade at :80, got {other:?}"),
	};
	let pred = &graph.predicates[pred_id.get() as usize];
	let dbg = format!("{pred:?}");
	assert!(dbg.contains("HttpUriPath"), "{dbg}");
	assert!(dbg.contains("acme-challenge"), "{dbg}");

	// And there should be an `acme-injected` annotation per affected listener.
	let injected: Vec<&_> =
		graph.meta.annotations.iter().filter(|a| a.kind == "acme-injected").collect();
	assert!(!injected.is_empty(), "expected at least one acme-injected annotation");
	assert!(
		injected.iter().any(|a| a.message.contains(":80")),
		"annotation must name the affected listener: {:?}",
		graph.meta.annotations,
	);
}

#[test]
fn inject_skips_https_port_443_listener() {
	// :443 is TLS-terminating, not plaintext — the inject pass must
	// leave it alone even when managed_snis is populated for it.
	let managed = managed_https_rule("api", ":443", "api.example.com");
	let plain80 = plain_http_rule("plain", ":80");
	let graph =
		compile(vec![rule_file(vec![managed, plain80])], &Providers, &Providers).expect("compile");
	let node = post_upgrade_node(&graph, entry_for_port(&graph, 443));
	if let Node::Check { predicate, .. } = node {
		let dbg = format!("{:?}", graph.predicates[predicate.get() as usize]);
		assert!(!dbg.contains("acme-challenge"), ":443 listener must not have ACME path inject");
	}
}

#[test]
fn inject_targets_only_port_80_not_arbitrary_plaintext() {
	// :8080 is plaintext but not :80 — the inject pass per spec
	// fires *only* on the well-known port 80 (CA validators query
	// the resolved IP at port 80, not custom ports).
	let plain8080 = plain_http_rule("plain", ":8080");
	let managed = managed_https_rule("api", ":443", "api.example.com");
	let graph =
		compile(vec![rule_file(vec![plain8080, managed])], &Providers, &Providers).expect("compile");
	let node = post_upgrade_node(&graph, entry_for_port(&graph, 8080));
	if let Node::Check { predicate, .. } = node {
		let dbg = format!("{:?}", graph.predicates[predicate.get() as usize]);
		assert!(!dbg.contains("acme-challenge"), ":8080 must not be touched by the inject pass");
	}
}

#[test]
fn injected_route_falls_through_to_original_entry_on_path_miss() {
	// The injected Check must keep the original listener entry as
	// its `on_miss` target so non-ACME traffic still flows through
	// the operator's rules.
	let plain80 = plain_http_rule("plain", ":80");
	let managed = managed_https_rule("api", ":443", "api.example.com");
	let graph =
		compile(vec![rule_file(vec![plain80, managed])], &Providers, &Providers).expect("compile");
	let node = post_upgrade_node(&graph, entry_for_port(&graph, 80));
	let on_miss = match node {
		Node::Check { on_miss, .. } => *on_miss,
		other => panic!("expected Check after Upgrade at :80, got {other:?}"),
	};
	// The on_miss target should be a real node (not the injected
	// fetch + terminator). Walk it: it should reach the operator's
	// http_synthesize fetch eventually.
	let target = &graph.nodes[on_miss.get() as usize];
	// At minimum, the target shouldn't itself be the AcmeChallenge fetch.
	if let Node::Fetch { id, .. } = target {
		let kind = graph.fetches[id.get() as usize].kind;
		assert_ne!(kind, FetchKind::AcmeChallenge, "on_miss must not loop back to the injected fetch");
	}
}

#[test]
fn injected_route_terminates_with_write_http_response() {
	// The fetch node injected by the inject pass must terminate
	// with a WriteHttpResponse — the AcmeChallenge fetch only
	// produces responses (no tunnel branch).
	let plain80 = plain_http_rule("plain", ":80");
	let managed = managed_https_rule("api", ":443", "api.example.com");
	let graph =
		compile(vec![rule_file(vec![plain80, managed])], &Providers, &Providers).expect("compile");
	let acme_fetch_node = graph
		.nodes
		.iter()
		.find_map(|n| match n {
			Node::Fetch { id, next_response, .. } => {
				let kind = graph.fetches[id.get() as usize].kind;
				if kind == FetchKind::AcmeChallenge { Some((id, *next_response)) } else { None }
			}
			_ => None,
		})
		.expect("inject must produce an AcmeChallenge fetch node");
	let next = acme_fetch_node.1.expect("AcmeChallenge fetch must have next_response");
	let term = match &graph.nodes[next.get() as usize] {
		Node::Terminate(t) => *t,
		other => panic!("expected Terminate after AcmeChallenge fetch, got {other:?}"),
	};
	assert_eq!(graph.terminators[term.get() as usize], Terminator::WriteHttpResponse);
}
