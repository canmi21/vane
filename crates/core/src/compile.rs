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
	use crate::ir::{Node, NodeId, PredicateId};
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
		let entries = rules.into_iter().map(crate::preset::RuleEntry::Raw).collect();
		RawRuleFile { path: PathBuf::from(path), order: 0, rules: entries }
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
		// Spec spec/flow-model.md § _Hash-consing_: predicates always dedup.
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
		// its own MiddlewareId per `spec/flow-model.md` § _Hash-consing_ — sharing buckets
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
		// Dry-run JSON contract (spec/flow-model.md § _The compiled form_): a compiled
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
					Node::Check {
						predicate: pa,
						on_match: ma,
						on_miss: sa,
						collect_body_before: ca,
						body_limit: la,
					},
					Node::Check {
						predicate: pb,
						on_match: mb,
						on_miss: sb,
						collect_body_before: cb,
						body_limit: lb,
					},
				) => {
					assert_eq!(pa, pb, "node[{i}] Check predicate");
					assert_eq!(ma, mb, "node[{i}] Check on_match");
					assert_eq!(sa, sb, "node[{i}] Check on_miss");
					assert_eq!(ca, cb, "node[{i}] Check collect_body_before");
					assert_eq!(la, lb, "node[{i}] Check body_limit");
				}
				(
					Node::Middleware {
						id: ia,
						next: na,
						on_error: ea,
						collect_body_before: ca,
						body_limit: la,
					},
					Node::Middleware {
						id: ib,
						next: nb,
						on_error: eb,
						collect_body_before: cb,
						body_limit: lb,
					},
				) => {
					assert_eq!(ia, ib, "node[{i}] Middleware id");
					assert_eq!(na, nb, "node[{i}] Middleware next");
					assert_eq!(ea, eb, "node[{i}] Middleware on_error");
					assert_eq!(ca, cb, "node[{i}] Middleware collect_body_before");
					assert_eq!(la, lb, "node[{i}] Middleware body_limit");
				}
				(
					Node::Fetch {
						id: ia,
						next_response: ra,
						next_tunnel: ta,
						collect_body_before: ca,
						body_limit: la,
					},
					Node::Fetch {
						id: ib,
						next_response: rb,
						next_tunnel: tb,
						collect_body_before: cb,
						body_limit: lb,
					},
				) => {
					assert_eq!(ia, ib, "node[{i}] Fetch id");
					assert_eq!(ra, rb, "node[{i}] Fetch next_response");
					assert_eq!(ta, tb, "node[{i}] Fetch next_tunnel");
					assert_eq!(ca, cb, "node[{i}] Fetch collect_body_before");
					assert_eq!(la, lb, "node[{i}] Fetch body_limit");
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

	// AnyOf / Not lowering tests
	fn check_rule(name: &str, port: u16, match_predicate: &serde_json::Value) -> RawRule {
		parse_rule(serde_json::json!({
			"name": name,
			"listen": [format!(":{port}")],
			"match": match_predicate,
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		}))
	}

	fn find_entry_check(graph: &SymbolicFlowGraph, port: u16) -> NodeId {
		let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
		let entry = *graph.entries.get(&v4).expect("entry present");
		// Per spec/flow-model.md § _The compiled form_, every L7
		// listener carries one shared Upgrade above the rule chains. Tests
		// that probe Check structure below the Upgrade skip past it here.
		match &graph[entry] {
			Node::Upgrade { next } => *next,
			_ => entry,
		}
	}

	fn unwrap_check(node: &Node) -> (PredicateId, NodeId, NodeId) {
		match node {
			Node::Check { predicate, on_match, on_miss, .. } => (*predicate, *on_match, *on_miss),
			other => panic!("expected Check, got {other:?}"),
		}
	}

	#[test]
	fn any_of_two_checks_chains_via_on_miss_sharing_on_match() {
		let r = check_rule(
			"r",
			7100,
			&serde_json::json!({
				"any_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "tls.sni": { "equals": "b" } },
				],
			}),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");

		let entry = find_entry_check(&graph, 7100);
		let (_, match_a, miss_a) = unwrap_check(&graph[entry]);
		let (_, match_b, _miss_b) = unwrap_check(&graph[miss_a]);
		assert_eq!(match_a, match_b, "both any_of branches share on_match");
		let check_count = graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count();
		assert_eq!(check_count, 2);
		assert_eq!(graph.predicates.len(), 2, "tls.sni=\"a\" and tls.sni=\"b\" are distinct");
	}

	#[test]
	fn any_of_three_checks_chains_right_to_left() {
		let r = check_rule(
			"r",
			7101,
			&serde_json::json!({
				"any_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "tls.sni": { "equals": "b" } },
					{ "tls.sni": { "equals": "c" } },
				],
			}),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");

		let c0 = find_entry_check(&graph, 7101);
		let (_, m0, miss0) = unwrap_check(&graph[c0]);
		let (_, m1, miss1) = unwrap_check(&graph[miss0]);
		let (_, m2, _miss2) = unwrap_check(&graph[miss1]);
		assert_eq!(m0, m1);
		assert_eq!(m1, m2, "all three any_of branches share on_match");
		assert_eq!(graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count(), 3);
	}

	#[test]
	fn not_wrapping_a_check_swaps_on_match_and_on_miss() {
		let r =
			check_rule("r", 7102, &serde_json::json!({ "not": { "tls.sni": { "equals": "internal" } } }));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");

		// Not adds no node — exactly one Check.
		let check_count = graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count();
		assert_eq!(check_count, 1);
		let entry = find_entry_check(&graph, 7102);
		let (_, on_match, on_miss) = unwrap_check(&graph[entry]);
		// Per the equivalence `not P match=>X miss=>Y` ≡ lower(P, match=>Y, miss=>X),
		// the emitted Check has swapped edges: its on_match is the outer on_miss
		// (the default-miss fallback) and its on_miss is the rule body entry.
		// Assert they're distinct — before-task-2 code had them both pointing
		// at the body entry.
		assert_ne!(on_match, on_miss);
		// Walking `on_miss` should land at something reachable; walking
		// `on_match` should land at a node that cannot reach the rule's Fetch.
		// Minimal structural check: the two targets differ.
	}

	#[test]
	fn not_wrapping_any_of_swaps_edges_and_produces_two_checks() {
		let r = check_rule(
			"r",
			7103,
			&serde_json::json!({
				"not": {
					"any_of": [
						{ "tls.sni": { "equals": "a" } },
						{ "tls.sni": { "equals": "b" } },
					],
				},
			}),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");

		assert_eq!(graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count(), 2);
		let c0 = find_entry_check(&graph, 7103);
		let (_, m0, miss0) = unwrap_check(&graph[c0]);
		let (_, m1, _miss1) = unwrap_check(&graph[miss0]);
		// `not (any_of [A, B])` = lower(any_of, match=>Y, miss=>X) =
		//   Check(A) match=>Y miss=>Check(B) match=>Y miss=>X.
		// Both Checks share on_match (== outer on_miss, i.e. the default-miss).
		assert_eq!(m0, m1);
	}

	#[test]
	fn any_of_nested_inside_any_of_produces_three_checks_with_shared_on_match() {
		let r = check_rule(
			"r",
			7104,
			&serde_json::json!({
				"any_of": [
					{ "tls.sni": { "equals": "a" } },
					{
						"any_of": [
							{ "tls.sni": { "equals": "b" } },
							{ "tls.sni": { "equals": "c" } },
						],
					},
				],
			}),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");

		let c0 = find_entry_check(&graph, 7104);
		let (_, m0, miss0) = unwrap_check(&graph[c0]);
		let (_, m1, miss1) = unwrap_check(&graph[miss0]);
		let (_, m2, _miss2) = unwrap_check(&graph[miss1]);
		assert_eq!(m0, m1);
		assert_eq!(m1, m2);
		assert_eq!(graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count(), 3);
	}

	#[test]
	fn empty_any_of_short_circuits_to_on_miss() {
		let r = check_rule("r", 7105, &serde_json::json!({ "any_of": [] }));
		// Empty any_of ≡ never matches. The rule's chain entry equals the
		// on_miss target (default-miss); no Check node is emitted for the
		// empty any_of itself.
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let check_count = graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count();
		assert_eq!(check_count, 0, "empty any_of must not emit a Check node");
	}

	#[test]
	fn any_of_hash_cons_shares_predicate_slot_across_rules() {
		// Two rules on different listeners both use the same `tls.sni ==
		// "shared"` predicate inside any_of. Per spec/flow-model.md § _Hash-consing_,
		// predicates dedup transparently regardless of the combinator tree
		// they're nested inside.
		let a = check_rule(
			"a",
			7106,
			&serde_json::json!({ "any_of": [{ "tls.sni": { "equals": "shared" } }] }),
		);
		let b = check_rule(
			"b",
			7107,
			&serde_json::json!({ "any_of": [{ "tls.sni": { "equals": "shared" } }] }),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers).expect("compile");
		assert_eq!(graph.predicates.len(), 1);
	}

	#[test]
	fn l4_predicate_on_l7_rule_sits_post_upgrade() {
		// Every L7 listener carries one shared Upgrade above the rule
		// chains, so L4-level Check leaves on L7 rules sit AFTER the
		// Upgrade. `PredicateView::L7Req` carries `conn`, so reads of
		// `tls.sni` / `remote.ip` remain functionally correct.
		let r = check_rule("r", 7300, &serde_json::json!({ "tls.sni": { "equals": "a" } }));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		// Listener entry is the shared Upgrade.
		let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7300);
		let listener_entry = *graph.entries.get(&v4).expect("entry present");
		assert!(matches!(&graph[listener_entry], Node::Upgrade { .. }));
		// One step below the Upgrade is the Check.
		let check_below = find_entry_check(&graph, 7300);
		assert!(matches!(&graph[check_below], Node::Check { .. }));
	}

	#[test]
	fn l7_predicate_on_l7_rule_sits_after_upgrade() {
		// Upgrade is listener-level rather than per-rule; L7-level Checks
		// still descend from it.
		let r = check_rule(
			"r",
			7301,
			&serde_json::json!({ "http.header.host": { "equals": "api.example.com" } }),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7301);
		let listener_entry = *graph.entries.get(&v4).expect("entry present");
		assert!(
			matches!(&graph[listener_entry], Node::Upgrade { .. }),
			"L7 listener entry is the shared Upgrade",
		);
		let Node::Upgrade { next } = &graph[listener_entry] else {
			panic!("expected Upgrade");
		};
		assert!(matches!(&graph[*next], Node::Check { .. }));
	}

	#[test]
	fn pure_l4_rule_with_predicate_synthesises_close_miss() {
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7302"],
			"match": { "remote.ip": { "cidr": "10.0.0.0/8" } },
			"terminate": { "type": "tcp_forward", "upstream": "10.0.0.5:22" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let entry = find_entry_check(&graph, 7302);
		assert!(matches!(&graph[entry], Node::Check { .. }));
		let upgrades = graph.nodes.iter().filter(|n| matches!(n, Node::Upgrade { .. })).count();
		assert_eq!(upgrades, 0, "L4 posture never Upgrades");
		assert!(
			graph.terminators.iter().any(|t| matches!(t, Terminator::Close)),
			"default-miss must synthesise a Close terminator",
		);
	}

	#[test]
	fn l7_rule_with_predicate_uses_close_not_500_for_default_miss() {
		// Pre-task-4 the L7 default-miss was a synthesised 500. Task 4 unified
		// both postures on `Terminator::Close`, so no HttpSynthesize fetch
		// should appear in the graph for a rule whose terminator is http_proxy.
		let r = check_rule("r", 7400, &serde_json::json!({ "tls.sni": { "equals": "api" } }));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		assert!(
			graph.terminators.iter().any(|t| matches!(t, Terminator::Close)),
			"default-miss must be Close",
		);
		let synth_fetches =
			graph.fetches.iter().filter(|f| f.kind == FetchKind::HttpSynthesize).count();
		assert_eq!(synth_fetches, 0, "no 500 synth for unmatched L7 traffic — just Close");
	}

	#[test]
	fn catch_all_rule_set_omits_close_fallback() {
		// A predicate-less rule is always matched; the default-miss is dead
		// code and must not appear in the graph.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7401"],
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let close_count = graph.terminators.iter().filter(|t| matches!(t, Terminator::Close)).count();
		assert_eq!(close_count, 0, "no predicate means no miss path means no Close");
	}

	#[test]
	fn close_terminator_serde_round_trip() {
		// Pin the wire form of the new variant for dry-run JSON.
		let t = Terminator::Close;
		let encoded = serde_json::to_string(&t).expect("serialize");
		let decoded: Terminator = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, t);
	}

	#[test]
	fn l7_rule_without_predicate_has_upgrade_as_entry() {
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7303"],
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7303);
		let listener_entry = *graph.entries.get(&v4).expect("entry present");
		assert!(matches!(&graph[listener_entry], Node::Upgrade { .. }));
	}

	#[test]
	fn cross_level_any_of_is_rejected() {
		let r = check_rule(
			"r",
			7304,
			&serde_json::json!({
				"any_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "http.method": { "equals": "GET" } },
				],
			}),
		);
		let err = compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers)
			.expect_err("cross-level any_of must fail");
		assert!(err.to_string().contains("cross-level"), "error message names the constraint: {err}");
	}

	#[test]
	fn cross_level_not_is_rejected() {
		let r = check_rule(
			"r",
			7305,
			&serde_json::json!({
				"not": {
					"any_of": [
						{ "tls.sni": { "equals": "a" } },
						{ "http.method": { "equals": "GET" } },
					],
				},
			}),
		);
		let err = compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers)
			.expect_err("cross-level not(any_of) must fail");
		assert!(err.to_string().contains("cross-level"));
	}

	#[test]
	fn same_level_any_of_compiles_at_one_side_of_upgrade() {
		// Two L4Peek checks: Upgrade sits BELOW both Checks.
		let r = check_rule(
			"r",
			7306,
			&serde_json::json!({
				"any_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "tls.sni": { "equals": "b" } },
				],
			}),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let entry = find_entry_check(&graph, 7306);
		// Entry is a Check; walking any_of's shared on_match must reach an Upgrade.
		assert!(matches!(&graph[entry], Node::Check { .. }));
	}

	#[test]
	fn validate_stays_green_for_all_combinator_shapes() {
		let shapes = [
			serde_json::json!({ "tls.sni": { "equals": "x" } }),
			serde_json::json!({
				"any_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "tls.sni": { "equals": "b" } },
				],
			}),
			serde_json::json!({ "not": { "tls.sni": { "equals": "y" } } }),
			serde_json::json!({
				"not": {
					"any_of": [
						{ "tls.sni": { "equals": "a" } },
						{ "tls.sni": { "equals": "b" } },
					],
				},
			}),
			serde_json::json!({
				"all_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "tls.sni": { "equals": "b" } },
				],
			}),
			serde_json::json!({
				"not": {
					"all_of": [
						{ "tls.sni": { "equals": "a" } },
						{ "tls.sni": { "equals": "b" } },
					],
				},
			}),
		];
		for (i, m) in shapes.iter().enumerate() {
			let port = 7200 + u16::try_from(i).expect("fits u16");
			let r = check_rule(&format!("r{i}"), port, m);
			let graph =
				compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
			validate::validate(&graph).expect("validate");
		}
	}

	#[test]
	fn all_of_two_checks_chains_left_match_to_right_entry() {
		// all_of [A, B] match=>X miss=>Y  ≡
		//   Check(A) match=>Check(B) match=>X miss=>Y, miss=>Y
		// Both Checks share on_miss; the first Check's on_match points at
		// the second Check's entry, not at X directly.
		let r = check_rule(
			"r",
			7500,
			&serde_json::json!({
				"all_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "tls.sni": { "equals": "b" } },
				],
			}),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let entry = find_entry_check(&graph, 7500);
		let (_, match_a, miss_a) = unwrap_check(&graph[entry]);
		let (_, _match_b, miss_b) = unwrap_check(&graph[match_a]);
		assert_eq!(miss_a, miss_b, "both all_of branches share on_miss");
		assert_eq!(graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count(), 2);
	}

	#[test]
	fn all_of_empty_array_short_circuits_to_on_match() {
		// `all_of: []` is vacuously true → no Check emitted, the rule's
		// chain entry equals on_match (the rule body).
		let r = check_rule("r", 7501, &serde_json::json!({ "all_of": [] }));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let check_count = graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count();
		assert_eq!(check_count, 0, "empty all_of must not emit a Check node");
	}

	#[test]
	fn all_of_cross_level_combinator_is_rejected() {
		// Same uniform-level rule as any_of: AllOf can't mix L4 and L7 leaves.
		let r = check_rule(
			"r",
			7502,
			&serde_json::json!({
				"all_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "http.method": { "equals": "GET" } },
				],
			}),
		);
		let err = compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers)
			.expect_err("cross-level all_of must fail");
		let msg = err.to_string();
		assert!(msg.contains("cross-level"), "error names the constraint: {msg}");
		assert!(msg.contains("all_of"), "error mentions all_of: {msg}");
	}

	#[test]
	fn all_of_nested_inside_any_of_works() {
		let r = check_rule(
			"r",
			7503,
			&serde_json::json!({
				"any_of": [
					{ "all_of": [
						{ "http.header.upgrade": { "equals": "websocket" } },
						{ "http.uri.path": { "prefix": "/ws" } },
					]},
					{ "all_of": [
						{ "http.header.upgrade": { "equals": "websocket" } },
						{ "http.uri.path": { "prefix": "/api/stream" } },
					]},
				],
			}),
		);
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		assert_eq!(graph.nodes.iter().filter(|n| matches!(n, Node::Check { .. })).count(), 4);
	}

	#[test]
	fn l7_listener_emits_single_upgrade_at_top_for_two_rules() {
		// spec/flow-model.md § _The compiled form_: two L7 rules on
		// the same listener share one Upgrade — the entire graph must
		// contain exactly one Upgrade node, not two.
		let a = parse_rule(serde_json::json!({
			"name": "a",
			"listen": [":7600"],
			"match": { "http.header.host": { "equals": "a.example.com" } },
			"terminate": { "type": "http_proxy" },
		}));
		let b = parse_rule(serde_json::json!({
			"name": "b",
			"listen": [":7600"],
			"match": { "http.header.host": { "equals": "b.example.com" } },
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers).expect("compile");
		let upgrades = graph.nodes.iter().filter(|n| matches!(n, Node::Upgrade { .. })).count();
		assert_eq!(upgrades, 1, "exactly one Upgrade per L7 listener regardless of rule count");
	}

	#[test]
	fn l4_listener_has_no_upgrade() {
		// L4 posture stays as-is — no Upgrade emitted.
		let r = parse_rule(serde_json::json!({
			"name": "fwd",
			"listen": [":7601"],
			"terminate": { "type": "tcp_forward", "upstream": "10.0.0.5:22" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let upgrades = graph.nodes.iter().filter(|n| matches!(n, Node::Upgrade { .. })).count();
		assert_eq!(upgrades, 0);
	}

	#[test]
	fn websocket_upgrade_emits_two_distinct_terminators() {
		// `WebSocketUpgrade` is dual-output — the response branch lands at
		// `WriteHttpResponse` (rejection); the tunnel branch lands at
		// `ByteTunnel` (101 Switching handoff).
		let r = parse_rule(serde_json::json!({
			"name": "ws",
			"listen": [":7602"],
			"match": { "http.header.upgrade": { "equals": "websocket" } },
			"terminate": { "type": "websocket", "upstream": "127.0.0.1:8080" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let terms: std::collections::HashSet<_> = graph.terminators.iter().copied().collect();
		assert!(terms.contains(&Terminator::WriteHttpResponse), "response branch terminator");
		assert!(terms.contains(&Terminator::ByteTunnel), "tunnel branch terminator");
	}

	// Short(Response) synth target
	#[test]
	fn lower_l7_listener_synthesizes_short_circuit_response_target() {
		// Every L7 listener gets a synth `Terminate(WriteHttpResponse)` that
		// `meta.short_circuit_response_entry` maps the listener entry to.
		// Without this synth, an L7 request middleware returning
		// `Decision::Short(ShortCircuit::Response(_))` has nowhere to land.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7700"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		// L7 listener → exactly one synth target per *unique* entry NodeId.
		// Dual-stack `:N` shorthand maps both v4 and v6 SocketAddrs to the
		// same listener NodeId, so the synth map keys by NodeId and is
		// smaller than `entries.len()` for the dual-stack case.
		let unique_entries: std::collections::HashSet<_> = graph.entries.values().copied().collect();
		assert_eq!(graph.meta.short_circuit_response_entry.len(), unique_entries.len());
		// Every value points at a `Terminate(WriteHttpResponse)`.
		for synth in graph.meta.short_circuit_response_entry.values() {
			let Node::Terminate(tid) = &graph[*synth] else {
				panic!("synth node is not a Terminate: {:?}", &graph[*synth]);
			};
			assert_eq!(graph.terminators[tid.get() as usize], Terminator::WriteHttpResponse);
		}
	}

	#[test]
	fn lower_l4_listener_has_no_short_circuit_response_target() {
		// Pure L4 (port_forward) listeners never go through Upgrade — no
		// L7Request middleware can run, so no synth target is needed.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7701"],
			"terminate": { "type": "tcp_forward", "upstream": "10.0.0.5:22" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		assert!(graph.meta.short_circuit_response_entry.is_empty());
	}

	#[test]
	fn lower_derives_raw_when_only_l4_forward_terminator() {
		// `port_forward`-shaped rule: single `tcp_forward` upstream on a
		// listener with no L7 path. Per `spec/crates/core.md` § _Listener kind
		// derivation_, every reachable terminator is L4 → `Raw`.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7800"],
			"terminate": { "type": "tcp_forward", "upstream": "10.0.0.5:22" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7800);
		assert_eq!(graph.meta.listener_kinds.get(&v4), Some(&crate::ir::ListenerKind::Raw));
	}

	#[test]
	fn lower_derives_http_when_only_l7_terminators() {
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7801"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7801);
		assert_eq!(graph.meta.listener_kinds.get(&v4), Some(&crate::ir::ListenerKind::Http));
	}

	#[test]
	fn lower_derives_http_for_udp_prefix_listener() {
		// `spec/crates/core.md` § _Listener kind derivation_ is graph-shape-only —
		// transport doesn't enter the rule. The `udp:` prefix flows
		// through `parse_listen` to `listener_transports`, while the
		// L7 terminator independently picks `Http`. The combination is
		// what the engine needs to spawn the per-listener H3 stack.
		let r = parse_rule(serde_json::json!({
			"name": "h3",
			"listen": ["udp:7802"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7802);
		assert_eq!(
			graph.meta.listener_kinds.get(&v4),
			Some(&crate::ir::ListenerKind::Http),
			"udp listener with http_proxy terminator must derive ListenerKind::Http",
		);
		assert_eq!(
			graph.meta.listener_transports.get(&v4),
			Some(&crate::conn_context::Transport::Udp),
			"udp: prefix must populate listener_transports as Udp",
		);
	}

	#[test]
	fn lower_derives_auto_when_l4_and_l7_share_listener() {
		// `analyze` currently rejects a single rule set with mixed L4 + L7
		// postures on the same listener (the protocol-detect frontend
		// that legitimises that combination is a separate feature). We
		// exercise the kind-derivation rule directly by hand-building a
		// graph whose entry can reach both an `L4Forward` fetch and an
		// `HttpProxy` fetch — the union of the two paths is what the
		// spec says yields `Auto`.
		use crate::compile::lower::test_only::derive_listener_kind_for_test;
		use crate::fetch::{FetchKind, SymbolicFetchRef};
		use crate::ir::{FetchId, ListenerKind, Node, NodeId, TerminatorId};

		let nodes = vec![
			Node::Check {
				predicate: PredicateId::new(0),
				on_match: NodeId::new(1),
				on_miss: NodeId::new(2),
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Fetch {
				id: FetchId::new(0),
				next_response: None,
				next_tunnel: Some(NodeId::new(3)),
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Fetch {
				id: FetchId::new(1),
				next_response: Some(NodeId::new(3)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			},
			Node::Terminate(TerminatorId::new(0)),
		];
		let fetches = vec![
			SymbolicFetchRef {
				kind: FetchKind::L4Forward,
				args: serde_json::Value::Null,
				retry_buffer_required: false,
				allow_zero_rtt: None,
			},
			SymbolicFetchRef {
				kind: FetchKind::HttpProxy,
				args: serde_json::Value::Null,
				retry_buffer_required: false,
				allow_zero_rtt: None,
			},
		];
		assert_eq!(derive_listener_kind_for_test(&nodes, &fetches, NodeId::new(0)), ListenerKind::Auto);
	}

	#[test]
	fn listener_kinds_round_trip_through_dry_run_json() {
		let l4 = parse_rule(serde_json::json!({
			"name": "tcp",
			"listen": [":7803"],
			"terminate": { "type": "tcp_forward", "upstream": "10.0.0.5:22" },
		}));
		let l7 = parse_rule(serde_json::json!({
			"name": "http",
			"listen": [":7804"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![l4, l7])], &Providers, &Providers).expect("compile");
		let encoded = serde_json::to_string(&*graph).expect("serialize graph");
		let decoded: crate::ir::SymbolicFlowGraph =
			serde_json::from_str(&encoded).expect("deserialize graph");
		let v4_raw = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7803);
		let v4_http = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7804);
		assert_eq!(decoded.meta.listener_kinds.get(&v4_raw), Some(&crate::ir::ListenerKind::Raw));
		assert_eq!(decoded.meta.listener_kinds.get(&v4_http), Some(&crate::ir::ListenerKind::Http));
	}

	#[test]
	fn lower_force_buffering_triggers_collect_body_before_request() {
		// `retry: { max_attempts: 3, buffering: "force" }` flags the
		// fetch node itself with `collect_body_before:
		// Some(BodySide::Request)` so the executor buffers the body
		// before the fetch runs. See `spec/crates/engine.md`
		// § _Retry_.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7900"],
			"terminate": {
				"type": "http_proxy",
				"upstream": "127.0.0.1:8080",
				"retry": { "max_attempts": 3, "buffering": "force" },
			},
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let fetch_with_collect = graph.nodes.iter().find_map(|n| match n {
			crate::ir::Node::Fetch {
				collect_body_before: Some(crate::ir::BodySide::Request), ..
			} => Some(()),
			_ => None,
		});
		assert!(
			fetch_with_collect.is_some(),
			"force buffering must flag fetch with collect_body_before"
		);
	}

	#[test]
	fn lower_opportunistic_buffering_does_not_trigger_collect() {
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7901"],
			"terminate": {
				"type": "http_proxy",
				"upstream": "127.0.0.1:8080",
				"retry": { "max_attempts": 3, "buffering": "opportunistic" },
			},
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let any_fetch_collects = graph.nodes.iter().any(|n| {
			matches!(
				n,
				crate::ir::Node::Fetch { collect_body_before: Some(crate::ir::BodySide::Request), .. },
			)
		});
		assert!(
			!any_fetch_collects,
			"opportunistic buffering must NOT flag fetch with collect_body_before",
		);
	}

	#[test]
	fn lower_max_attempts_one_with_force_does_not_trigger_collect() {
		// `force` only means anything together with `max_attempts > 1`.
		// Pinning `max_attempts: 1` keeps retry off and the buffering
		// trigger inactive even when the JSON spells `force`.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7902"],
			"terminate": {
				"type": "http_proxy",
				"upstream": "127.0.0.1:8080",
				"retry": { "max_attempts": 1, "buffering": "force" },
			},
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let any_fetch_collects = graph.nodes.iter().any(|n| {
			matches!(
				n,
				crate::ir::Node::Fetch { collect_body_before: Some(crate::ir::BodySide::Request), .. },
			)
		});
		assert!(!any_fetch_collects, "max_attempts=1 disables the force-buffering trigger");
	}

	#[test]
	fn lower_two_l7_listeners_have_independent_synth_entries() {
		// Two L7 listeners on distinct ports each get their own synth
		// target keyed by their listener entry. (Whether the synth nodes
		// collapse via terminator-id hash-cons is an internal detail; the
		// public contract is one map entry per listener entry.)
		let a = parse_rule(serde_json::json!({
			"name": "a",
			"listen": [":7702"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		}));
		let b = parse_rule(serde_json::json!({
			"name": "b",
			"listen": [":7703"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8081" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![a, b])], &Providers, &Providers).expect("compile");
		// One synth entry per listener entry — the dual-stack `:7702`
		// expands to v4+v6 sharing one entry NodeId, so map size matches
		// the number of *unique* entry NodeIds in the graph.
		let unique_entries: std::collections::HashSet<_> = graph.entries.values().copied().collect();
		assert_eq!(graph.meta.short_circuit_response_entry.len(), unique_entries.len());
		assert!(
			graph.meta.short_circuit_response_entry.len() >= 2,
			"two listeners → at least two synth entries"
		);
	}

	#[test]
	fn http_body_check_node_sets_collect_body_before_request() {
		// A rule whose predicate reads http.body must produce a Check node
		// with collect_body_before = Some(BodySide::Request). The executor
		// uses this flag to materialise the request body before running the
		// predicate test — without it, as_static() would panic.
		//
		// "aGVsbG8=" is standard base64 for the ASCII bytes "hello".
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7910"],
			"match": { "http.body": { "contains": "aGVsbG8=" } },
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let has_collecting_check = graph.nodes.iter().any(|n| {
			matches!(
				n,
				crate::ir::Node::Check { collect_body_before: Some(crate::ir::BodySide::Request), .. }
			)
		});
		assert!(has_collecting_check, "http.body Check must carry collect_body_before = Some(Request)");
	}

	#[test]
	fn rule_without_http_body_predicate_has_no_request_collect_on_check() {
		// A rule whose predicate does not read http.body must not trigger
		// request-side buffering. The Check node produced for an
		// http.method predicate must have collect_body_before = None so the
		// pay-as-you-go invariant holds.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7911"],
			"match": { "http.method": { "equals": "GET" } },
			"terminate": { "type": "http_proxy" },
		}));
		let graph =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect("compile");
		let any_check_collects = graph.nodes.iter().any(|n| {
			matches!(
				n,
				crate::ir::Node::Check { collect_body_before: Some(crate::ir::BodySide::Request), .. }
			)
		});
		assert!(
			!any_check_collects,
			"non-body predicate must not set collect_body_before on any Check node"
		);
	}

	#[test]
	fn malformed_base64_in_http_body_predicate_fails_compile() {
		// The base64 literal in a Bytes-typed field predicate is decoded at
		// lower time, not at predicate-test time. A syntactically invalid
		// base64 string must produce a compile-time error so the rule never
		// reaches the runtime graph. "not-valid-base64!!!" contains '!'
		// which is not in the standard base64 alphabet.
		let r = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":7912"],
			"match": { "http.body": { "contains": "not-valid-base64!!!" } },
			"terminate": { "type": "http_proxy" },
		}));
		let err =
			compile(vec![rule_file("a.json", vec![r])], &Providers, &Providers).expect_err("must fail");
		let msg = err.to_string();
		assert!(
			msg.contains("base64") || msg.contains("base 64"),
			"error must mention base64 decoding failure, got: {msg}"
		);
	}
}
