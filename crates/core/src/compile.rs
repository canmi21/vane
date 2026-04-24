pub mod analyze;
pub mod expand;
pub mod lower;
pub mod merge;
pub mod validate;

use std::sync::Arc;

use crate::error::Error;
use crate::ir::SymbolicFlowGraph;
use crate::metadata::{FetchMetadataProvider, MiddlewareMetadataProvider};

pub use analyze::{AnalyzedRule, AnalyzedRuleSet, InspectionLevel, Posture};
pub use expand::RawRuleSet;
pub use merge::{MergedConfig, RawRuleFile};

/// Facade for the core compile pipeline.
///
/// Runs `merge → expand → analyze → lower → validate` and returns an
/// `Arc<SymbolicFlowGraph>` ready for `vane-engine::FlowGraph::link`.
///
/// # Errors
/// Returns [`Error::compile`] on duplicate rule names, unknown middleware
/// or fetch names referenced by rules, bad `ListenSpec` strings, predicate
/// type mismatches, or graph-level validation failures (dangling IDs,
/// cycles, phase mismatches).
pub fn compile(
	files: Vec<RawRuleFile>,
	mw_meta: &dyn MiddlewareMetadataProvider,
	fetch_meta: &dyn FetchMetadataProvider,
) -> Result<Arc<SymbolicFlowGraph>, Error> {
	let merged = merge::merge(files)?;
	let expanded = expand::expand(merged)?;
	let analyzed = analyze::analyze(expanded, mw_meta, fetch_meta)?;
	let graph = lower::lower(analyzed, mw_meta, fetch_meta)?;
	validate::validate(&graph)?;
	Ok(Arc::new(graph))
}

#[cfg(test)]
mod tests {
	use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
	use std::path::PathBuf;

	use super::*;
	use crate::fetch::{FetchKind, FetchOutputModes, FetchPhase, Terminator};
	use crate::ir::Node;
	use crate::metadata::{FetchMetadata, MiddlewareMetadata};
	use crate::middleware::MiddlewareKind;
	use crate::rule::{RawRule, TerminateSpec};

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

	fn parse_rule(j: serde_json::Value) -> RawRule {
		serde_json::from_value(j).expect("parse rule")
	}

	fn rule_file(path: &str, rules: Vec<RawRule>) -> RawRuleFile {
		RawRuleFile { path: PathBuf::from(path), order: 0, rules }
	}

	fn _unused_mentions() {
		let _ = TerminateSpec { kind: FetchKind::HttpProxy, args: serde_json::Value::Null };
	}

	#[test]
	fn reverse_proxy_end_to_end_compiles_with_dual_stack_entries() {
		let r = parse_rule(serde_json::json!({
			"name": "proxy",
			"listen": [":443"],
			"middleware_chain": [{ "use": "forward_client_ip" }, { "use": "rate_limit", "args": { "rate": 100 } }],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		}));
		let graph =
			compile(vec![rule_file("30-proxy.json", vec![r])], &Providers, &Providers).expect("compile");
		assert!(!graph.nodes.is_empty());
		// Dual-stack `:443` expands to both v4 and v6 SocketAddrs sharing one entry NodeId.
		let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443);
		let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 443);
		let e_v4 = graph.entries.get(&v4).expect("v4 entry present");
		let e_v6 = graph.entries.get(&v6).expect("v6 entry present");
		assert_eq!(e_v4, e_v6);
		// The terminator set contains WriteHttpResponse (both the rule terminator
		// and the synthesised default-miss write it).
		assert!(
			graph.terminators.iter().any(|t| matches!(t, Terminator::WriteHttpResponse)),
			"expected WriteHttpResponse terminator",
		);
	}

	#[test]
	fn predicate_hash_cons_shares_id_across_rules() {
		// Two rules on different listeners both match `tls.sni == "api"`.
		// Spec 02-flow.md § _Hash-consing_: predicates always dedup.
		let a = parse_rule(serde_json::json!({
			"name": "a",
			"listen": [":8443"],
			"match": { "tls.sni": { "equals": "api" } },
			"terminate": { "type": "http_proxy" },
		}));
		let b = parse_rule(serde_json::json!({
			"name": "b",
			"listen": [":9443"],
			"match": { "tls.sni": { "equals": "api" } },
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers).expect("compile");
		assert_eq!(graph.predicates.len(), 1, "identical predicates must hash-cons to one slot");
	}

	#[test]
	fn stateless_middleware_hash_cons_across_rules() {
		// Two rules sharing an identical `forward_client_ip` (stateless, no args)
		// must share one MiddlewareId.
		let a = parse_rule(serde_json::json!({
			"name": "a",
			"listen": [":7001"],
			"middleware_chain": [{ "use": "forward_client_ip" }],
			"terminate": { "type": "http_proxy" },
		}));
		let b = parse_rule(serde_json::json!({
			"name": "b",
			"listen": [":7002"],
			"middleware_chain": [{ "use": "forward_client_ip" }],
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers).expect("compile");
		let shared = graph
			.middlewares
			.iter()
			.filter(|m| m.name.as_ref() == "forward_client_ip" && m.stateless)
			.count();
		assert_eq!(shared, 1, "stateless middleware dedups across rules");
	}

	#[test]
	fn stateful_middleware_per_site_not_shared() {
		// Two rules both use `rate_limit` (stateful). Each call site must get
		// its own MiddlewareId per spec § _Hash-consing_ — sharing buckets
		// would silently halve the effective rate.
		let a = parse_rule(serde_json::json!({
			"name": "a",
			"listen": [":7003"],
			"middleware_chain": [{ "use": "rate_limit", "args": { "rate": 100 } }],
			"terminate": { "type": "http_proxy" },
		}));
		let b = parse_rule(serde_json::json!({
			"name": "b",
			"listen": [":7004"],
			"middleware_chain": [{ "use": "rate_limit", "args": { "rate": 100 } }],
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers).expect("compile");
		let rate_limit_count =
			graph.middlewares.iter().filter(|m| m.name.as_ref() == "rate_limit").count();
		assert_eq!(rate_limit_count, 2, "stateful middleware must not share ids across call sites");
	}

	#[test]
	fn terminator_variant_derives_from_fetch_kind() {
		// HttpProxy / HttpSynthesize → WriteHttpResponse; L4Forward → ByteTunnel.
		let http = parse_rule(serde_json::json!({
			"name": "http",
			"listen": [":8080"],
			"terminate": { "type": "http_proxy" },
		}));
		let tcp = parse_rule(serde_json::json!({
			"name": "tcp",
			"listen": [":2222"],
			"terminate": { "type": "tcp_forward", "upstream": "10.0.0.5:22" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![http, tcp])], &Providers, &Providers).expect("compile");
		let terms: std::collections::HashSet<_> = graph.terminators.iter().copied().collect();
		assert!(terms.contains(&Terminator::WriteHttpResponse));
		assert!(terms.contains(&Terminator::ByteTunnel));
	}

	#[test]
	fn l7_rule_inserts_upgrade_node() {
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let upgrades = graph.nodes.iter().filter(|n| matches!(n, Node::Upgrade { .. })).count();
		assert!(upgrades >= 1, "L7 listener must have at least one Upgrade node");
	}

	#[test]
	fn l4_only_rule_has_no_upgrade() {
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":2222"],
			"terminate": { "type": "tcp_forward", "upstream": "10.0.0.5:22" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let upgrades = graph.nodes.iter().filter(|n| matches!(n, Node::Upgrade { .. })).count();
		assert_eq!(upgrades, 0);
	}

	#[test]
	fn duplicate_rule_names_fail_at_merge_stage() {
		let a = parse_rule(serde_json::json!({
			"name": "same",
			"listen": [":1000"],
			"terminate": { "type": "http_proxy" },
		}));
		let b = parse_rule(serde_json::json!({
			"name": "same",
			"listen": [":1001"],
			"terminate": { "type": "http_proxy" },
		}));
		let err = compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers)
			.expect_err("duplicate must fail");
		assert!(err.to_string().contains("duplicate"));
	}

	#[test]
	fn any_of_combinator_is_not_supported_in_this_chunk() {
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"match": {
				"any_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "tls.sni": { "equals": "b" } },
				],
			},
			"terminate": { "type": "http_proxy" },
		}));
		let err = compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers)
			.expect_err("any_of not yet lowered");
		assert!(err.to_string().contains("any_of"));
	}

	#[test]
	fn wildcard_port_listen_spec_is_rejected() {
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":0"],
			"terminate": { "type": "http_proxy" },
		}));
		let err = compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers)
			.expect_err("wildcard port must fail");
		assert!(err.to_string().contains("wildcard port"));
	}

	#[test]
	fn validate_runs_and_catches_basic_graph_integrity() {
		// End-to-end: `compile` runs `validate` inside. A clean reverse_proxy
		// graph must pass — this is an end-to-end sanity check that validate
		// is wired into the pipeline and doesn't falsely reject good graphs.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		// Running validate again on the returned graph must still succeed.
		validate::validate(&graph).expect("re-validate");
	}

	#[test]
	fn symbolic_flow_graph_round_trip_preserves_structure_and_revalidates() {
		// Dry-run JSON contract (02-flow.md § _The compiled form_): a compiled
		// SymbolicFlowGraph serializes to JSON and the result deserializes
		// back to an equivalent graph that re-`validate()`s green. Slab
		// contents and `entries` map key set must survive the round-trip.
		use crate::ir::SymbolicFlowGraph;
		let r = parse_rule(serde_json::json!({
			"name": "proxy",
			"listen": [":443"],
			"middleware_chain": [{ "use": "forward_client_ip" }, { "use": "rate_limit", "args": { "rate": 100 } }],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");

		let encoded = serde_json::to_string(&*graph).expect("serialize graph");
		let decoded: SymbolicFlowGraph = serde_json::from_str(&encoded).expect("deserialize graph");

		// Re-validate the decoded graph: the contract is that dry-run JSON
		// is a ground-truth snapshot that the engine could rehydrate.
		validate::validate(&decoded).expect("decoded graph revalidates");

		// Slab lengths survive.
		assert_eq!(decoded.nodes.len(), graph.nodes.len(), "nodes slab length");
		assert_eq!(decoded.predicates.len(), graph.predicates.len(), "predicates slab length");
		assert_eq!(decoded.middlewares.len(), graph.middlewares.len(), "middlewares slab length");
		assert_eq!(decoded.fetches.len(), graph.fetches.len(), "fetches slab length");
		assert_eq!(decoded.terminators.len(), graph.terminators.len(), "terminators slab length");

		// `entries` key set (SocketAddr → NodeId) survives.
		let orig_keys: std::collections::BTreeSet<_> = graph.entries.keys().copied().collect();
		let dec_keys: std::collections::BTreeSet<_> = decoded.entries.keys().copied().collect();
		assert_eq!(orig_keys, dec_keys, "entries key set must round-trip");

		// PredicateInst / SymbolicMiddlewareRef / Terminator implement
		// PartialEq; compare their slabs directly.
		assert_eq!(decoded.predicates, graph.predicates, "predicates slab content");
		assert_eq!(decoded.middlewares, graph.middlewares, "middlewares slab content");
		assert_eq!(decoded.terminators, graph.terminators, "terminators slab content");

		// `Node` does not implement PartialEq (by design — the enum holds
		// id newtypes and Option<NodeId>s only). Compare node-by-node via
		// variant destructuring to pin that the control-flow structure
		// survived the round-trip.
		for (i, (a, b)) in graph.nodes.iter().zip(decoded.nodes.iter()).enumerate() {
			match (a, b) {
				(
					Node::Check { predicate: pa, on_match: ma, on_miss: sa, collect_body_before: ca },
					Node::Check { predicate: pb, on_match: mb, on_miss: sb, collect_body_before: cb },
				) => {
					assert_eq!(pa, pb, "node[{i}] Check predicate");
					assert_eq!(ma, mb, "node[{i}] Check on_match");
					assert_eq!(sa, sb, "node[{i}] Check on_miss");
					assert_eq!(ca, cb, "node[{i}] Check collect_body_before");
				}
				(
					Node::Middleware { id: ia, next: na, on_error: ea, collect_body_before: ca },
					Node::Middleware { id: ib, next: nb, on_error: eb, collect_body_before: cb },
				) => {
					assert_eq!(ia, ib, "node[{i}] Middleware id");
					assert_eq!(na, nb, "node[{i}] Middleware next");
					assert_eq!(ea, eb, "node[{i}] Middleware on_error");
					assert_eq!(ca, cb, "node[{i}] Middleware collect_body_before");
				}
				(
					Node::Fetch { id: ia, next_response: ra, next_tunnel: ta, collect_body_before: ca },
					Node::Fetch { id: ib, next_response: rb, next_tunnel: tb, collect_body_before: cb },
				) => {
					assert_eq!(ia, ib, "node[{i}] Fetch id");
					assert_eq!(ra, rb, "node[{i}] Fetch next_response");
					assert_eq!(ta, tb, "node[{i}] Fetch next_tunnel");
					assert_eq!(ca, cb, "node[{i}] Fetch collect_body_before");
				}
				(Node::Upgrade { next: a }, Node::Upgrade { next: b }) => {
					assert_eq!(a, b, "node[{i}] Upgrade next");
				}
				(Node::Terminate(a), Node::Terminate(b)) => {
					assert_eq!(a, b, "node[{i}] Terminate");
				}
				(a, b) => panic!("node[{i}] variant changed across round-trip: {a:?} -> {b:?}"),
			}
		}
	}
}
