use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};

use crate::compile::analyze::{AnalyzedRule, AnalyzedRuleSet, Posture};
use crate::conn_context::Transport;
use crate::error::Error;
use crate::fetch::{FetchKind, FetchPhase, SymbolicFetchRef, Terminator};
use crate::ir::{
	BodySide, FetchId, FlowGraphMeta, ListenerKind, MiddlewareId, Node, NodeId, PredicateId,
	SymbolicFlowGraph, TerminatorId,
};
use crate::metadata::{FetchMetadataProvider, MiddlewareMetadataProvider};
use crate::middleware::{MiddlewareKind, SymbolicMiddlewareRef};
use crate::predicate::{
	CompiledOperator, CompiledValue, FieldPath, FieldValueType, Operator, Predicate, PredicateInst,
	Value,
};
use crate::rule::SourceInfo;

/// Lower an analyzed rule set to a `SymbolicFlowGraph`.
///
/// # Errors
/// Returns [`Error::compile`] for unknown middleware / fetch names, invalid
/// predicate shapes (`AnyOf` / `Not` deferred past this chunk), unresolvable
/// `ListenSpec` strings, predicate-value type mismatches against their field
/// path, and rule sets that mix L4 and L7 posture on one listener without a
/// catch-all fallback.
pub fn lower(
	set: AnalyzedRuleSet,
	mw_meta: &dyn MiddlewareMetadataProvider,
	fetch_meta: &dyn FetchMetadataProvider,
) -> Result<SymbolicFlowGraph, Error> {
	let version_hash = hash_rules(&set.rules);
	let mut builder = Builder::new();

	let groups = group_by_listener(&set.rules)?;
	for (transport, addrs, rules) in groups {
		// TLS termination is per-listener, not per-rule: every rule
		// sharing an address contributes to the listener's cert pool.
		// `resolve_listener_tls` aggregates and rejects conflicts —
		// see 08-tls.md § _TLS termination_ + § _Certificate resolver_.
		let resolved_tls = resolve_listener_tls(&addrs, &rules)?;
		// Per-rule `allow_zero_rtt` checks (presence + idempotent-method
		// gate) live alongside the TLS aggregation since they reference
		// the listener-level posture (TLS-L7 vs plaintext / L4) that
		// `resolve_listener_tls` already established. See `08-tls.md`
		// § _TLS 1.3 0-RTT (early data)_ § _Compile-time constraints_.
		validate_zero_rtt_for_listener(&addrs, &rules, resolved_tls.as_ref())?;
		let entry = builder.lower_port(&rules, mw_meta, fetch_meta)?;
		for addr in &addrs {
			builder.entries.insert(*addr, entry);
		}
		if let Some(spec) = resolved_tls {
			for addr in &addrs {
				builder.listener_tls.insert(*addr, spec.clone());
			}
		}
		let kind = derive_listener_kind(&builder.nodes, &builder.fetches, entry);
		// Listener transport comes from the parsed `tcp:` / `udp:`
		// prefix on `listen` (09-config.md § _ListenSpec grammar_).
		// `validate_listener_fetches` walks the entry subgraph and
		// rejects any L4Forward whose `args.transport` disagrees with
		// the declared listener transport.
		validate_listener_fetches(&addrs, transport, &builder.nodes, &builder.fetches, entry)?;
		for addr in addrs {
			builder.listener_kinds.insert(addr, kind);
			builder.listener_transports.insert(addr, transport);
		}
	}

	// Per `spec/acme.md` § _HTTP-01 § Compile-time checks_: when any
	// rule declares a `tls.managed.challenge == "http-01"` SNI but
	// the operator has no plaintext `:80` listener anywhere in the
	// config, the daemon will auto-bind one at runtime. Emit a
	// compile-time WARN so this is visible without waiting for
	// runtime telemetry. The check runs after the listener loop so
	// `builder.listener_kinds` / `listener_tls` are fully populated.
	warn_missing_plaintext_port_80_for_http01(&builder.listener_tls, &builder.listener_kinds);

	Ok(SymbolicFlowGraph {
		nodes: builder.nodes,
		predicates: builder.predicates,
		middlewares: builder.middlewares,
		fetches: builder.fetches,
		terminators: builder.terminators,
		entries: builder.entries,
		meta: FlowGraphMeta {
			version_hash,
			compiled_at: SystemTime::now(),
			source_files: set.source_files,
			feature_set: &[],
			short_circuit_response_entry: builder.short_circuit_response_entry,
			listener_tls: builder.listener_tls,
			listener_kinds: builder.listener_kinds,
			listener_transports: builder.listener_transports,
		},
	})
}

/// Walk the entry subgraph collecting every reachable
/// [`SymbolicFetchRef`]. Map each to a [`FetchPhase`] via
/// [`FetchKind::phase`] and pick the [`ListenerKind`] from the
/// resulting set per `06-l4.md` § _Listener kind derivation_:
///
/// | reachable phases     | derived kind |
/// | -------------------- | ------------ |
/// | only `L4`            | `Raw`        |
/// | only `L7`            | `Http`       |
/// | both `L4` and `L7`   | `Auto`       |
///
/// A graph with no fetches reachable from `entry` would defeat
/// validate.rs (every entry must terminate), so the empty-set fallback
/// to `Http` here is purely defensive — it never fires on a legal
/// rule set.
fn derive_listener_kind(
	nodes: &[Node],
	fetches: &[SymbolicFetchRef],
	entry: NodeId,
) -> ListenerKind {
	let mut seen_l4 = false;
	let mut seen_l7 = false;
	let mut visited = std::collections::HashSet::new();
	let mut queue = std::collections::VecDeque::from([entry]);
	while let Some(id) = queue.pop_front() {
		if !visited.insert(id) {
			continue;
		}
		let Some(node) = nodes.get(id.get() as usize) else { continue };
		match node {
			Node::Check { on_match, on_miss, .. } => {
				queue.push_back(*on_match);
				queue.push_back(*on_miss);
			}
			Node::Middleware { next, on_error, .. } => {
				queue.push_back(*next);
				if let Some(e) = on_error {
					queue.push_back(*e);
				}
			}
			Node::Fetch { id, next_response, next_tunnel, .. } => {
				match fetches[id.get() as usize].kind.phase() {
					FetchPhase::L4 => seen_l4 = true,
					FetchPhase::L7 => seen_l7 = true,
				}
				if let Some(n) = next_response {
					queue.push_back(*n);
				}
				if let Some(n) = next_tunnel {
					queue.push_back(*n);
				}
			}
			Node::Upgrade { next } => queue.push_back(*next),
			Node::Terminate(_) => {}
		}
	}
	match (seen_l4, seen_l7) {
		(true, true) => ListenerKind::Auto,
		(false, true) => ListenerKind::Http,
		// `(true, false)` is the strict spec rule (only-L4 → Raw);
		// `(false, false)` is the defensive arm for fetch-less
		// hand-built test fixtures (peek-only → Close). Both collapse
		// to `Raw` so the L4 subgraph walk still fires.
		(true | false, false) => ListenerKind::Raw,
	}
}

/// Walk every reachable `L4Forward` fetch under `entry` and reject
/// any whose `args.transport` disagrees with the listener's declared
/// transport.
///
/// Per `09-config.md` § _`ListenSpec` grammar_ the listener prefix is
/// authoritative: a `tcp:` listener with a UDP `L4Forward`, or a
/// `udp:` listener with a TCP `L4Forward`, is a hard compile error.
/// `L7` fetches and `L4Forward` whose `args.transport` is unset
/// (defaults to TCP) on a TCP listener are silently accepted.
///
/// `addrs` is purely for the error message — the listener's identity
/// in operator-facing diagnostics.
///
/// # Errors
///
/// Returns [`Error::compile`] when any reachable `L4Forward` carries
/// `args.transport` opposite the listener transport. The error names
/// the listener address(es), the declared listener transport, and
/// the offending fetch's upstream so operators can locate the
/// conflicting rule.
fn validate_listener_fetches(
	addrs: &[SocketAddr],
	listener_transport: Transport,
	nodes: &[Node],
	fetches: &[SymbolicFetchRef],
	entry: NodeId,
) -> Result<(), Error> {
	let mut visited = std::collections::HashSet::new();
	let mut queue = std::collections::VecDeque::from([entry]);
	while let Some(id) = queue.pop_front() {
		if !visited.insert(id) {
			continue;
		}
		let Some(node) = nodes.get(id.get() as usize) else { continue };
		match node {
			Node::Check { on_match, on_miss, .. } => {
				queue.push_back(*on_match);
				queue.push_back(*on_miss);
			}
			Node::Middleware { next, on_error, .. } => {
				queue.push_back(*next);
				if let Some(e) = on_error {
					queue.push_back(*e);
				}
			}
			Node::Fetch { id, next_response, next_tunnel, .. } => {
				let fetch = &fetches[id.get() as usize];
				if matches!(fetch.kind, FetchKind::L4Forward) {
					let fetch_transport =
						match fetch.args.get("transport").and_then(serde_json::Value::as_str) {
							Some("udp") => Some(Transport::Udp),
							Some("tcp") | None => Some(Transport::Tcp),
							Some(other) => {
								return Err(Error::compile(format!(
									"listener {addrs:?}: L4Forward fetch carries unknown transport {other:?}",
								)));
							}
						};
					if let Some(ft) = fetch_transport
						&& ft != listener_transport
					{
						let upstream =
							fetch.args.get("upstream").and_then(serde_json::Value::as_str).unwrap_or("<unknown>");
						return Err(Error::compile(format!(
							"listener {addrs:?} declared {listener_transport:?} but reachable L4Forward (upstream {upstream:?}) carries transport {ft:?} — listener prefix and fetch transport must agree",
						)));
					}
				}
				if let Some(n) = next_response {
					queue.push_back(*n);
				}
				if let Some(n) = next_tunnel {
					queue.push_back(*n);
				}
			}
			Node::Upgrade { next } => queue.push_back(*next),
			Node::Terminate(_) => {}
		}
	}
	Ok(())
}

/// Peek at a fetch's `args.retry` JSON to decide whether the lower
/// pass needs to flag the fetch node with `collect_body_before:
/// Some(BodySide::Request)`. Returns `true` only when the policy
/// has `max_attempts > 1` and `buffering: "force"`. The full retry
/// schema is parsed by the engine's fetch factory; this helper is
/// the minimum the lower pass needs to thread the buffering decision
/// through to the graph shape.
///
/// `args` is the entire fetch args object; the helper looks for the
/// `retry` sub-object and tolerates its absence.
fn peek_retry_buffer_required(args: &serde_json::Value) -> bool {
	let Some(retry) = args.get("retry") else {
		return false;
	};
	let max_attempts = retry.get("max_attempts").and_then(serde_json::Value::as_u64).unwrap_or(1);
	if max_attempts <= 1 {
		return false;
	}
	let buffering =
		retry.get("buffering").and_then(serde_json::Value::as_str).unwrap_or("opportunistic");
	buffering == "force"
}

#[cfg(test)]
pub(crate) mod test_only {
	use std::net::SocketAddr;

	use super::{
		Error, ListenerKind, Node, NodeId, SymbolicFetchRef, Transport, derive_listener_kind,
		parse_listen, validate_listener_fetches,
	};

	/// Test escape hatch for the upstream `compile.rs::tests` module —
	/// exposes the derivation helper without leaking it to non-test
	/// callers.
	pub(crate) fn derive_listener_kind_for_test(
		nodes: &[Node],
		fetches: &[SymbolicFetchRef],
		entry: NodeId,
	) -> ListenerKind {
		derive_listener_kind(nodes, fetches, entry)
	}

	pub(crate) fn parse_listen_for_test(spec: &str) -> Result<(Transport, Vec<SocketAddr>), Error> {
		parse_listen(spec)
	}

	pub(crate) fn validate_listener_fetches_for_test(
		addrs: &[SocketAddr],
		listener_transport: Transport,
		nodes: &[Node],
		fetches: &[SymbolicFetchRef],
		entry: NodeId,
	) -> Result<(), Error> {
		validate_listener_fetches(addrs, listener_transport, nodes, fetches, entry)
	}
}

#[cfg(test)]
mod listen_parse_tests {
	use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

	use super::test_only::parse_listen_for_test;
	use crate::conn_context::Transport;

	fn parse(s: &str) -> (Transport, Vec<SocketAddr>) {
		parse_listen_for_test(s).expect("parse listen ok")
	}

	#[test]
	fn bare_dual_stack_defaults_to_tcp() {
		let (t, addrs) = parse(":443");
		assert_eq!(t, Transport::Tcp);
		assert_eq!(
			addrs,
			vec![
				SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443),
				SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 443),
			]
		);
	}

	#[test]
	fn bare_specific_v4_defaults_to_tcp() {
		let (t, addrs) = parse("0.0.0.0:443");
		assert_eq!(t, Transport::Tcp);
		assert_eq!(addrs, vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443)]);
	}

	#[test]
	fn bare_specific_v6_defaults_to_tcp() {
		let (t, addrs) = parse("[::]:443");
		assert_eq!(t, Transport::Tcp);
		assert_eq!(addrs, vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 443)]);
	}

	#[test]
	fn tcp_prefix_dual_stack_yields_tcp() {
		let (t, addrs) = parse("tcp:443");
		assert_eq!(t, Transport::Tcp);
		assert_eq!(addrs.len(), 2, "dual-stack expansion preserved under prefix");
	}

	#[test]
	fn udp_prefix_dual_stack_yields_udp() {
		let (t, addrs) = parse("udp:443");
		assert_eq!(t, Transport::Udp);
		assert_eq!(addrs.len(), 2, "dual-stack expansion preserved under prefix");
	}

	#[test]
	fn tcp_prefix_specific_v4_address() {
		let (t, addrs) = parse("tcp:0.0.0.0:443");
		assert_eq!(t, Transport::Tcp);
		assert_eq!(addrs, vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 443)]);
	}

	#[test]
	fn udp_prefix_v6_unspecified() {
		let (t, addrs) = parse("udp:[::]:443");
		assert_eq!(t, Transport::Udp);
		assert_eq!(addrs, vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 443)]);
	}

	#[test]
	fn tcp_prefix_v6_specific_loopback() {
		let (t, addrs) = parse("tcp:[::1]:443");
		assert_eq!(t, Transport::Tcp);
		assert_eq!(addrs, vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 443)]);
	}

	#[test]
	fn uppercase_prefix_rejected() {
		// `TCP:` is not the recognized lowercase prefix; falls through to
		// the address parser, which fails on the leading non-IP token.
		let err = parse_listen_for_test("TCP:443").expect_err("uppercase prefix must reject");
		assert!(err.to_string().contains("bad listen spec"), "{err}");
	}

	#[test]
	fn unknown_prefix_rejected() {
		// `udpx:` is not a known prefix; the address parser then fails
		// on the leading `udpx` IP-token.
		let err = parse_listen_for_test("udpx:443").expect_err("unknown prefix must reject");
		assert!(err.to_string().contains("bad listen spec"), "{err}");
	}

	#[test]
	fn udp_prefix_with_zero_port_rejected() {
		// `udp::0` strips to `:0`, which the wildcard-port guard rejects
		// per the spec lock.
		let err = parse_listen_for_test("udp::0").expect_err("port 0 must reject");
		assert!(err.to_string().contains("wildcard port rejected"), "{err}");
	}

	#[test]
	fn tcp_prefix_with_zero_port_rejected() {
		let err = parse_listen_for_test("tcp::0").expect_err("port 0 must reject");
		assert!(err.to_string().contains("wildcard port rejected"), "{err}");
	}

	#[test]
	fn udp_double_colon_strips_one_prefix() {
		// `udp::443` → strip leading `udp:`, parse `:443` as a bare
		// dual-stack port (the inner `:` is part of the address form).
		let (t, addrs) = parse("udp::443");
		assert_eq!(t, Transport::Udp);
		assert_eq!(addrs.len(), 2);
		assert_eq!(addrs[0].port(), 443);
	}
}

#[cfg(test)]
mod listener_fetch_validation_tests {
	use std::net::SocketAddr;
	use std::str::FromStr as _;

	use super::test_only::validate_listener_fetches_for_test;
	use crate::conn_context::Transport;
	use crate::fetch::{FetchKind, SymbolicFetchRef, Terminator};
	use crate::ir::{FetchId, Node, NodeId, TerminatorId};

	fn fetch_node(id: u32, term: u32) -> Node {
		Node::Fetch {
			id: FetchId::new(id),
			next_response: None,
			next_tunnel: Some(NodeId::new(term)),
			collect_body_before: None,
			body_limit: 0,
		}
	}

	fn l4_fetch(transport: &str) -> SymbolicFetchRef {
		SymbolicFetchRef {
			kind: FetchKind::L4Forward,
			args: serde_json::json!({ "upstream": "127.0.0.1:9", "transport": transport }),
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}
	}

	fn l7_fetch() -> SymbolicFetchRef {
		SymbolicFetchRef {
			kind: FetchKind::HttpProxy,
			args: serde_json::json!({ "upstream": "127.0.0.1:9" }),
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}
	}

	fn addr() -> Vec<SocketAddr> {
		vec![SocketAddr::from_str("0.0.0.0:443").expect("addr")]
	}

	#[test]
	fn udp_listener_with_udp_l4_forward_passes() {
		let nodes = vec![fetch_node(0, 1), Node::Terminate(TerminatorId::new(0))];
		let fetches = vec![l4_fetch("udp")];
		validate_listener_fetches_for_test(&addr(), Transport::Udp, &nodes, &fetches, NodeId::new(0))
			.expect("udp listener + udp L4Forward must pass");
	}

	#[test]
	fn tcp_listener_with_tcp_l4_forward_passes() {
		let nodes = vec![fetch_node(0, 1), Node::Terminate(TerminatorId::new(0))];
		let fetches = vec![l4_fetch("tcp")];
		validate_listener_fetches_for_test(&addr(), Transport::Tcp, &nodes, &fetches, NodeId::new(0))
			.expect("tcp listener + tcp L4Forward must pass");
	}

	#[test]
	fn tcp_listener_with_l4_forward_default_transport_passes() {
		// `args.transport` absent defaults to TCP at the fetch layer.
		let nodes = vec![fetch_node(0, 1), Node::Terminate(TerminatorId::new(0))];
		let fetches = vec![SymbolicFetchRef {
			kind: FetchKind::L4Forward,
			args: serde_json::json!({ "upstream": "127.0.0.1:9" }),
			retry_buffer_required: false,
			allow_zero_rtt: None,
		}];
		validate_listener_fetches_for_test(&addr(), Transport::Tcp, &nodes, &fetches, NodeId::new(0))
			.expect("tcp listener + default-transport L4Forward must pass");
	}

	#[test]
	fn tcp_listener_with_udp_l4_forward_compile_errors() {
		let nodes = vec![fetch_node(0, 1), Node::Terminate(TerminatorId::new(0))];
		let fetches = vec![l4_fetch("udp")];
		let err =
			validate_listener_fetches_for_test(&addr(), Transport::Tcp, &nodes, &fetches, NodeId::new(0))
				.expect_err("tcp listener + udp L4Forward must reject");
		let msg = err.to_string();
		assert!(msg.contains("0.0.0.0:443"), "error names listener address: {msg}");
		assert!(msg.contains("Tcp"), "error names listener transport: {msg}");
		assert!(msg.contains("Udp"), "error names fetch transport: {msg}");
		assert!(msg.contains("127.0.0.1:9"), "error names offending fetch: {msg}");
	}

	#[test]
	fn udp_listener_with_tcp_l4_forward_compile_errors() {
		let nodes = vec![fetch_node(0, 1), Node::Terminate(TerminatorId::new(0))];
		let fetches = vec![l4_fetch("tcp")];
		let err =
			validate_listener_fetches_for_test(&addr(), Transport::Udp, &nodes, &fetches, NodeId::new(0))
				.expect_err("udp listener + tcp L4Forward must reject");
		let msg = err.to_string();
		assert!(msg.contains("0.0.0.0:443"), "error names listener address: {msg}");
		assert!(msg.contains("Udp"), "error names listener transport: {msg}");
		assert!(msg.contains("Tcp"), "error names fetch transport: {msg}");
	}

	#[test]
	fn udp_listener_with_l7_only_passes() {
		// Only L7 fetches reachable — no fetch transport to conflict
		// with the listener's UDP prefix. Listener kind derivation will
		// pick `Http` (= H3), but that's `derive_listener_kind`'s job.
		let nodes = vec![
			Node::Upgrade { next: NodeId::new(1) },
			fetch_node(0, 2),
			Node::Terminate(TerminatorId::new(0)),
		];
		let fetches = vec![l7_fetch()];
		let _ = Terminator::WriteHttpResponse;
		validate_listener_fetches_for_test(&addr(), Transport::Udp, &nodes, &fetches, NodeId::new(0))
			.expect("udp listener + l7-only must pass (kind derivation handles Http)");
	}

	#[test]
	fn udp_listener_with_mixed_l4_branches_compile_errors() {
		// Branch on Check: one arm L4Forward(udp), the other
		// L4Forward(tcp). The TCP branch under a UDP listener fails.
		let nodes = vec![
			Node::Check {
				predicate: crate::ir::PredicateId::new(0),
				on_match: NodeId::new(1),
				on_miss: NodeId::new(3),
				collect_body_before: None,
				body_limit: 0,
			},
			fetch_node(0, 2),
			Node::Terminate(TerminatorId::new(0)),
			fetch_node(1, 2),
		];
		let fetches = vec![l4_fetch("udp"), l4_fetch("tcp")];
		let err =
			validate_listener_fetches_for_test(&addr(), Transport::Udp, &nodes, &fetches, NodeId::new(0))
				.expect_err("udp listener + mixed L4 branches must reject");
		assert!(err.to_string().contains("must agree"), "{err}");
	}
}

struct Builder {
	nodes: Vec<Node>,
	predicates: Vec<PredicateInst>,
	pred_dedup: HashMap<PredicateInst, PredicateId>,
	middlewares: Vec<SymbolicMiddlewareRef>,
	mw_dedup: HashMap<(String, String), MiddlewareId>,
	fetches: Vec<SymbolicFetchRef>,
	terminators: Vec<Terminator>,
	term_dedup: HashMap<Terminator, TerminatorId>,
	entries: HashMap<SocketAddr, NodeId>,
	/// L7 listener entry `NodeId` → synthesised
	/// `Terminate(WriteHttpResponse)` `NodeId`. Populated by `lower_port`
	/// for each listener that emits an `Upgrade`; consumed by the
	/// executor when a request middleware returns `Short(Response(_))`.
	/// See spec/architecture/02-flow.md § _`FlowGraph` metadata_.
	short_circuit_response_entry: std::collections::BTreeMap<NodeId, NodeId>,
	/// Per-listener cert pool (symbolic). Populated by `resolve_listener_tls`
	/// after aggregating every rule's `tls` block on this address; the
	/// engine's `link` parses each entry into a `rustls::ServerConfig`.
	/// See spec/architecture/08-tls.md § _TLS termination_.
	listener_tls: std::collections::BTreeMap<SocketAddr, crate::rule::ListenerTlsSpec>,
	/// Per-listener dispatch posture (symbolic). Populated as
	/// `lower_port` finishes each address group; see
	/// [`derive_listener_kind`] for the rule.
	listener_kinds: std::collections::BTreeMap<SocketAddr, ListenerKind>,
	/// Per-listener wire transport. Populated as `lower_port` finishes
	/// each address group; see [`derive_listener_transport`] for the
	/// derivation rule and conflict semantics.
	listener_transports: std::collections::BTreeMap<SocketAddr, Transport>,
}

impl Builder {
	fn new() -> Self {
		Self {
			nodes: Vec::new(),
			predicates: Vec::new(),
			pred_dedup: HashMap::new(),
			middlewares: Vec::new(),
			mw_dedup: HashMap::new(),
			fetches: Vec::new(),
			terminators: Vec::new(),
			term_dedup: HashMap::new(),
			entries: HashMap::new(),
			short_circuit_response_entry: std::collections::BTreeMap::new(),
			listener_tls: std::collections::BTreeMap::new(),
			listener_kinds: std::collections::BTreeMap::new(),

			listener_transports: std::collections::BTreeMap::new(),
		}
	}

	fn intern_predicate(&mut self, p: PredicateInst) -> PredicateId {
		if let Some(&id) = self.pred_dedup.get(&p) {
			return id;
		}
		let id = PredicateId::new(u32::try_from(self.predicates.len()).expect("predicate id fits u32"));
		self.predicates.push(p.clone());
		self.pred_dedup.insert(p, id);
		id
	}

	fn intern_middleware(&mut self, r: SymbolicMiddlewareRef) -> MiddlewareId {
		if r.stateless {
			let key = (r.name.to_string(), canonical_json(&r.args));
			if let Some(&id) = self.mw_dedup.get(&key) {
				return id;
			}
			let id =
				MiddlewareId::new(u32::try_from(self.middlewares.len()).expect("middleware id fits u32"));
			self.middlewares.push(r);
			self.mw_dedup.insert(key, id);
			id
		} else {
			let id =
				MiddlewareId::new(u32::try_from(self.middlewares.len()).expect("middleware id fits u32"));
			self.middlewares.push(r);
			id
		}
	}

	fn push_fetch(&mut self, r: SymbolicFetchRef) -> FetchId {
		let id = FetchId::new(u32::try_from(self.fetches.len()).expect("fetch id fits u32"));
		self.fetches.push(r);
		id
	}

	fn intern_terminator(&mut self, t: Terminator) -> TerminatorId {
		if let Some(&id) = self.term_dedup.get(&t) {
			return id;
		}
		let id =
			TerminatorId::new(u32::try_from(self.terminators.len()).expect("terminator id fits u32"));
		self.terminators.push(t);
		self.term_dedup.insert(t, id);
		id
	}

	fn push_node(&mut self, n: Node) -> NodeId {
		let id = NodeId::new(u32::try_from(self.nodes.len()).expect("node id fits u32"));
		self.nodes.push(n);
		id
	}

	fn lower_port(
		&mut self,
		rules: &[&AnalyzedRule],
		mw_meta: &dyn MiddlewareMetadataProvider,
		fetch_meta: &dyn FetchMetadataProvider,
	) -> Result<NodeId, Error> {
		let posture = rules.first().map_or(Posture::L7, |r| r.posture);
		if rules.iter().any(|r| r.posture != posture) {
			return Err(Error::compile(
				"mixed L4 and L7 rules on one listener require protocol_detect (S1-16); not in this chunk"
					.to_string(),
			));
		}

		// Sort: inspection level desc, specificity desc, name asc.
		let mut ordered: Vec<&AnalyzedRule> = rules.to_vec();
		ordered.sort_by(|a, b| {
			b.inspection_level
				.cmp(&a.inspection_level)
				.then(b.specificity.cmp(&a.specificity))
				.then(a.raw.name.cmp(&b.raw.name))
		});

		// Synthesize a default-miss only when at least one rule has a
		// predicate that could miss and thus needs a fallback target. A set
		// of catch-all (predicate-less) rules produces a chain whose entry
		// is the first rule's first node — the default-miss is dead code.
		// Both L4 and L7 postures terminate the miss path in `Terminator::Close`
		// per 05-terminator.md § _Variants_ C5.5 update: unmatched traffic
		// is silently dropped (port scans, protocol probes, misroutes).
		let needs_fallback = ordered.iter().any(|r| r.raw.match_predicate.is_some());
		let fallback_miss =
			if needs_fallback { self.synthesize_default_miss() } else { NodeId::new(0) };

		// Build the inner chain (no per-rule Upgrade). For an L7 listener
		// we wrap the resulting entry in ONE shared `Node::Upgrade` below.
		// 02-flow.md § _Listener-level Upgrade placement_: emitting one
		// Upgrade per rule and stitching them via on_miss puts the second
		// Upgrade in `Phase::L7Request`, which the validator rejects. A
		// single listener-level Upgrade keeps every cross-rule on_miss edge
		// in the post-Upgrade phase.
		let mut current_miss = fallback_miss;
		for rule in ordered.iter().rev() {
			let chain_entry = self.lower_rule(rule, current_miss, mw_meta, fetch_meta)?;
			current_miss = chain_entry;
		}
		let inner_entry = current_miss;

		match posture {
			Posture::L7 => {
				// Synthesize a `Terminate(WriteHttpResponse)` so an L7 request
				// middleware that returns `Short(ShortCircuit::Response(_))`
				// has somewhere to land. The executor sets the response slot
				// on the `Decision::Short` arm and jumps to this synth target;
				// the standard `WriteHttpResponse` write path emits the bytes.
				// See spec/architecture/02-flow.md § _`FlowGraph` metadata_.
				//
				// The map key is `inner_entry` — the node Upgrade's `next`
				// points at — *not* the listener-level Upgrade NodeId.
				// Reason: `drive_h1_server` re-enters `execute` with the
				// post-Upgrade entry as the `entry` parameter (see
				// `executor.rs::Node::Upgrade` arm: `drive_h1_server(stream,
				// graph, *next, ...)`), and the executor's
				// Short(Response) arm looks the synth target up by *that*
				// `entry`. Keying by the Upgrade NodeId would miss every
				// real lookup.
				let synth_tid = self.intern_terminator(Terminator::WriteHttpResponse);
				let synth_node = self.push_node(Node::Terminate(synth_tid));
				let listener_entry = self.push_node(Node::Upgrade { next: inner_entry });
				self.short_circuit_response_entry.insert(inner_entry, synth_node);
				Ok(listener_entry)
			}
			Posture::L4 => Ok(inner_entry),
		}
	}

	fn synthesize_default_miss(&mut self) -> NodeId {
		// Unified across postures: unmatched traffic silently drops via
		// `Terminator::Close`. Operators who want a branded HTTP error for
		// unmatched L7 requests add an explicit catch-all rule with
		// `type: "static"` (HttpSynthesize) — spec 05-terminator.md.
		let tid = self.intern_terminator(Terminator::Close);
		self.push_node(Node::Terminate(tid))
	}

	fn lower_rule(
		&mut self,
		rule: &AnalyzedRule,
		on_miss: NodeId,
		mw_meta: &dyn MiddlewareMetadataProvider,
		fetch_meta: &dyn FetchMetadataProvider,
	) -> Result<NodeId, Error> {
		// Build tail-first so on_* edges point at already-allocated NodeIds.
		// `WebSocketUpgrade` is dual-output: the response branch emits a
		// WriteHttpResponse terminator (for rejection / 4xx), the tunnel
		// branch emits a ByteTunnel terminator (for the 101-Switching
		// handoff). Single-output fetches reuse one terminator node on the
		// active branch only.
		let fetch_kind = rule.raw.terminate.kind;
		let retry_buffer_required = peek_retry_buffer_required(&rule.raw.terminate.args);
		let fid = self.push_fetch(SymbolicFetchRef {
			kind: fetch_kind,
			args: rule.raw.terminate.args.clone(),
			retry_buffer_required,
			// Lift the rule's `allow_zero_rtt` onto the per-rule fetch so
			// the executor's `Node::Fetch` arm can consult it without a
			// rule-side lookup. `None` here means the rule's listener is
			// not TLS-terminating L7 — the runtime gate is unreachable.
			// The lower pass has already validated the field's presence
			// matches the listener type via `validate_zero_rtt_for_rule`.
			allow_zero_rtt: rule.raw.allow_zero_rtt,
		});
		let (next_response, next_tunnel) = match fetch_kind {
			FetchKind::HttpProxy | FetchKind::HttpSynthesize => {
				let tid = self.intern_terminator(Terminator::WriteHttpResponse);
				let term_node = self.push_node(Node::Terminate(tid));
				(Some(term_node), None)
			}
			FetchKind::L4Forward => {
				let tid = self.intern_terminator(Terminator::ByteTunnel);
				let term_node = self.push_node(Node::Terminate(tid));
				(None, Some(term_node))
			}
			FetchKind::WebSocketUpgrade => {
				let resp_tid = self.intern_terminator(Terminator::WriteHttpResponse);
				let resp_node = self.push_node(Node::Terminate(resp_tid));
				let tun_tid = self.intern_terminator(Terminator::ByteTunnel);
				let tun_node = self.push_node(Node::Terminate(tun_tid));
				(Some(resp_node), Some(tun_node))
			}
		};
		let _ = fetch_meta;
		let fetch_node_idx = self.nodes.len();
		let fetch_node_id = NodeId::new(u32::try_from(fetch_node_idx).expect("node id fits u32"));
		// `buffering: "force"` on a `max_attempts > 1` retry policy
		// flags the fetch node itself with `collect_body_before:
		// Some(BodySide::Request)` — the executor reads this at node
		// entry, so by the time the fetch runs the body has been
		// drained from the upstream `Body::Stream` into a
		// `Body::Static` snapshot the retry loop can replay. See
		// `spec/architecture/05-terminator.md` § _Retry buffering_.
		let (fetch_collect, fetch_body_limit) = if retry_buffer_required {
			(Some(BodySide::Request), rule.raw.max_body_bytes_request)
		} else {
			(None, 0)
		};
		self.nodes.push(Node::Fetch {
			id: fid,
			next_response,
			next_tunnel,
			collect_body_before: fetch_collect,
			body_limit: fetch_body_limit,
		});

		// Middleware chain, reverse-linked so each `next` points at the
		// already-emitted successor.
		let mut head = fetch_node_id;
		let mut req_first_reader_seen = false;
		let mut resp_first_reader_seen = false;
		// Walk chain in reverse so we can attach `next` edges to already-placed nodes.
		for mw_ref in rule.raw.middleware_chain.iter().rev() {
			let meta = mw_meta
				.get(&mw_ref.name)
				.ok_or_else(|| Error::compile(format!("unknown middleware: {:?}", mw_ref.name)))?;
			let sym = SymbolicMiddlewareRef {
				name: Arc::from(mw_ref.name.as_str()),
				args: mw_ref.args.clone(),
				kind: meta.kind,
				stateless: meta.stateless,
				needs_body: meta.needs_body,
				on_error: None,
			};
			let id = self.intern_middleware(sym);
			let node = Node::Middleware {
				id,
				next: head,
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			};
			head = self.push_node(node);
		}

		// Second pass (forward): place LazyBuffer first-reader flags. We walk
		// from the chain's entry (head) forward to fetch, flagging the first
		// node that reads the body on each side.
		let chain_entry_before_upgrade = head;
		let _ = (&mut req_first_reader_seen, &mut resp_first_reader_seen);
		if rule.needs_request_body {
			self.mark_request_reader(
				chain_entry_before_upgrade,
				mw_meta,
				rule.raw.max_body_bytes_request,
			)?;
		}
		if rule.needs_response_body {
			self.mark_response_reader(
				chain_entry_before_upgrade,
				mw_meta,
				rule.raw.max_body_bytes_response,
			)?;
		}

		// Validate the predicate's leaves are uniform-level (cross-level
		// combinators still rejected per C5.5 § _Predicate uniform level_).
		// Placement no longer depends on level — the listener-level Upgrade
		// (added by `lower_port`) sits above the entire inner chain, so every
		// Check sits in the post-Upgrade phase regardless of leaf level.
		// PredicateView's `L7Req` variant carries `conn`, so L4-only fields
		// (`remote.ip`, `tls.sni`) remain readable here.
		//
		// SPEC DEVIATION (intentional, documented in 02-flow.md § _Listener-
		// level Upgrade placement_): the C5.5-era "L4-level Check fails fast
		// before HTTP decode" optimisation is lost — L7 listeners now decode
		// the request before evaluating L4-level predicates. See spec for
		// the trade-off.
		let _ = rule.raw.match_predicate.as_ref().map(predicate_uniform_level).transpose()?;

		if let Some(pred) = &rule.raw.match_predicate {
			head = self.lower_predicate(pred, head, on_miss, &rule.raw.source)?;
		}

		Ok(head)
	}

	fn lower_predicate(
		&mut self,
		pred: &Predicate,
		on_match: NodeId,
		on_miss: NodeId,
		source: &SourceInfo,
	) -> Result<NodeId, Error> {
		match pred {
			Predicate::Check(c) => {
				let inst =
					PredicateInst { path: c.path.clone(), op: compile_operator(&c.op, &c.path, source)? };
				let pid = self.intern_predicate(inst);
				let collect_body_before =
					if matches!(c.path, FieldPath::HttpBody) { Some(BodySide::Request) } else { None };
				let node =
					Node::Check { predicate: pid, on_match, on_miss, collect_body_before, body_limit: 0 };
				Ok(self.push_node(node))
			}
			Predicate::AnyOf(any_of) => {
				// any_of [A, B, C] match=>X miss=>Y  ≡
				//     Check(A) match=>X miss=>Check(B) match=>X miss=>Check(C) match=>X miss=>Y
				// Build right-to-left so each preceding Check's on_miss points
				// at the next child's entry.
				if any_of.any_of.is_empty() {
					// Empty any_of is an empty OR — always misses (vacuous false).
					return Ok(on_miss);
				}
				let mut cur_miss = on_miss;
				for child in any_of.any_of.iter().rev() {
					cur_miss = self.lower_predicate(child, on_match, cur_miss, source)?;
				}
				Ok(cur_miss)
			}
			Predicate::AllOf(all_of) => {
				// all_of [A, B, C] match=>X miss=>Y  ≡
				//     Check(A) match=>Check(B) match=>Check(C) match=>X miss=>Y, miss=>Y, miss=>Y
				// Dual of AnyOf: chain `on_match` forward through children; the
				// shared `on_miss` short-circuits the whole conjunction.
				if all_of.all_of.is_empty() {
					// Empty all_of is an empty AND — always matches (vacuous true).
					return Ok(on_match);
				}
				let mut cur_match = on_match;
				for child in all_of.all_of.iter().rev() {
					cur_match = self.lower_predicate(child, cur_match, on_miss, source)?;
				}
				Ok(cur_match)
			}
			Predicate::Not(not) => {
				// not P match=>X miss=>Y  ≡  lower(P, match=>Y, miss=>X)
				self.lower_predicate(&not.not, on_miss, on_match, source)
			}
		}
	}

	fn mark_request_reader(
		&mut self,
		chain_head: NodeId,
		_mw_meta: &dyn MiddlewareMetadataProvider,
		body_limit: usize,
	) -> Result<(), Error> {
		// Walk from chain_head forward through Middleware / Fetch nodes and
		// flag the first reader on the request side. A Check reading http.body
		// is handled inline in `lower_rule`; this covers the middleware case.
		let mut cur = chain_head;
		loop {
			match &self.nodes[cur.get() as usize] {
				Node::Middleware { id, next, .. } => {
					let sym = &self.middlewares[id.get() as usize];
					let is_req_reader = sym.kind == MiddlewareKind::L7Request && sym.needs_body;
					let next_id = *next;
					if is_req_reader {
						if let Node::Middleware { collect_body_before, body_limit: bl, .. } =
							&mut self.nodes[cur.get() as usize]
						{
							*collect_body_before = Some(BodySide::Request);
							*bl = body_limit;
						}
						return Ok(());
					}
					cur = next_id;
				}
				Node::Fetch { .. } | Node::Terminate(_) | Node::Upgrade { .. } | Node::Check { .. } => {
					return Ok(());
				}
			}
		}
	}

	fn mark_response_reader(
		&mut self,
		chain_head: NodeId,
		_mw_meta: &dyn MiddlewareMetadataProvider,
		body_limit: usize,
	) -> Result<(), Error> {
		// Walk from chain_head forward through Middleware nodes, find the first
		// L7Response middleware that needs_body, and flag it.
		let mut cur = chain_head;
		loop {
			match &self.nodes[cur.get() as usize] {
				Node::Middleware { id, next, .. } => {
					let sym = &self.middlewares[id.get() as usize];
					let is_resp_reader = sym.kind == MiddlewareKind::L7Response && sym.needs_body;
					let next_id = *next;
					if is_resp_reader {
						if let Node::Middleware { collect_body_before, body_limit: bl, .. } =
							&mut self.nodes[cur.get() as usize]
						{
							*collect_body_before = Some(BodySide::Response);
							*bl = body_limit;
						}
						return Ok(());
					}
					cur = next_id;
				}
				Node::Fetch { .. } | Node::Terminate(_) | Node::Upgrade { .. } | Node::Check { .. } => {
					return Ok(());
				}
			}
		}
	}
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum Level {
	L4Only,
	L4Peek,
	L7Header,
	L7Body,
}

fn field_path_level(path: &FieldPath) -> Level {
	match path {
		FieldPath::Transport
		| FieldPath::RemoteIp
		| FieldPath::RemotePort
		| FieldPath::LocalIp
		| FieldPath::LocalPort => Level::L4Only,
		FieldPath::Peek
		| FieldPath::TlsSni
		| FieldPath::TlsAlpn
		| FieldPath::TlsVersion
		| FieldPath::TlsPeerCertPresent
		| FieldPath::TlsPeerCertSubjectCn
		| FieldPath::TlsPeerCertSanDns
		| FieldPath::TlsPeerCertFingerprintSha256
		| FieldPath::TlsPeerCertSpkiSha256
		| FieldPath::TlsPeerCertIssuerCn
		| FieldPath::TlsPeerCertSerial => Level::L4Peek,
		FieldPath::HttpMethod
		| FieldPath::HttpUriPath
		| FieldPath::HttpUriQuery
		| FieldPath::HttpHeader(_) => Level::L7Header,
		FieldPath::HttpBody => Level::L7Body,
	}
}

const fn level_is_l4(l: Level) -> bool {
	matches!(l, Level::L4Only | Level::L4Peek)
}

/// Walk a predicate subtree and return the single level common to every
/// Check leaf. Combinators that mix L4 and L7 leaves are rejected so the
/// resulting graph's Check placement (before vs. after Upgrade) stays
/// unambiguous. Empty combinators have no leaves and yield the lowest
/// level (`L4Only`) — they never emit a Check, so the value is unused.
fn predicate_uniform_level(pred: &Predicate) -> Result<Level, Error> {
	let mut acc: Option<Level> = None;
	collect_levels(pred, &mut acc)?;
	Ok(acc.unwrap_or(Level::L4Only))
}

fn collect_levels(pred: &Predicate, acc: &mut Option<Level>) -> Result<(), Error> {
	match pred {
		Predicate::Check(c) => {
			let leaf = field_path_level(&c.path);
			match *acc {
				None => *acc = Some(leaf),
				Some(existing) if level_is_l4(existing) == level_is_l4(leaf) => {
					if (leaf as u8) > (existing as u8) {
						*acc = Some(leaf);
					}
				}
				Some(existing) => {
					return Err(Error::compile(format!(
						"cross-level any_of / all_of / not not supported: predicate mixes {existing:?} and {leaf:?} leaves"
					)));
				}
			}
			Ok(())
		}
		Predicate::AnyOf(a) => {
			for child in &a.any_of {
				collect_levels(child, acc)?;
			}
			Ok(())
		}
		Predicate::AllOf(a) => {
			for child in &a.all_of {
				collect_levels(child, acc)?;
			}
			Ok(())
		}
		Predicate::Not(n) => collect_levels(&n.not, acc),
	}
}

#[allow(dead_code)]
fn predicate_is_l4(pred: Option<&Predicate>) -> bool {
	let Some(Predicate::Check(c)) = pred else {
		return false;
	};
	matches!(
		c.path,
		FieldPath::Transport
			| FieldPath::RemoteIp
			| FieldPath::RemotePort
			| FieldPath::LocalIp
			| FieldPath::LocalPort
			| FieldPath::Peek
			| FieldPath::TlsSni
			| FieldPath::TlsAlpn
			| FieldPath::TlsVersion
			| FieldPath::TlsPeerCertPresent
			| FieldPath::TlsPeerCertSubjectCn
			| FieldPath::TlsPeerCertSanDns
			| FieldPath::TlsPeerCertFingerprintSha256
			| FieldPath::TlsPeerCertSpkiSha256
			| FieldPath::TlsPeerCertIssuerCn
			| FieldPath::TlsPeerCertSerial
	)
}

type ListenerGroup<'a> = (Transport, Vec<SocketAddr>, Vec<&'a AnalyzedRule>);

/// Per-listener TLS resolution — aggregate every rule's `tls` block
/// into a `ListenerTlsSpec` cert pool.
///
/// Each rule with `tls = Some(_)` contributes one cert into the pool,
/// keyed by `tls.sni` (lowercased ASCII per 08-tls.md § _SNI
/// normalization_). `sni: None` is the listener's _default_ — at most
/// one is allowed.
///
/// Returns `Ok(None)` when no rule on this listener carries TLS
/// (cleartext listener). Errors when:
///
/// - Two rules declare a default cert (sni-less) with different
///   `cert_file` / `key_file`: a listener has at most one default.
/// - Two rules declare the same SNI with different cert files: the
///   resolver can't pick deterministically.
/// - Any rule on a pure-L4 listener carries `tls`: TLS termination on
///   a byte-tunnel makes no sense — vane decrypts the client's TLS,
///   then forwards plaintext to the upstream. Either omit `tls`, or
///   change the terminator to an L7 type.
///
/// Hash-cons: completely identical `(sni, cert_file, key_file)`
/// triples across rules are deduped (e.g. two rules on the same
/// listener that point at the same cert paths share one pool entry).
/// Route a single `TlsConfig` into the right per-listener bucket
/// (`default` / `sni_certs` / `managed_snis`) on `spec`. Conflict
/// detection — same SNI declared twice with different specs, or
/// declared as both static and managed — is centralised here so
/// `resolve_listener_tls` stays under the clippy line cap.
///
/// Pre-condition: `tls.validate()` has already passed (enforced by
/// `analyze::analyze_rule`).
fn route_tls_config_into_spec(
	addrs: &[SocketAddr],
	tls: &crate::rule::TlsConfig,
	spec: &mut crate::rule::ListenerTlsSpec,
) -> Result<(), Error> {
	if let Some(managed) = tls.managed.as_ref() {
		let sni_key =
			tls.sni.as_deref().expect("managed validated requires tls.sni").to_ascii_lowercase();
		if spec.sni_certs.contains_key(&sni_key) {
			return Err(Error::compile(format!(
				"listener {addrs:?}: SNI {sni_key:?} declared as both static and managed — pick one source"
			)));
		}
		match spec.managed_snis.get(&sni_key) {
			None => {
				spec.managed_snis.insert(sni_key, managed.clone());
			}
			Some(existing) if existing == managed => {}
			Some(_) => {
				return Err(Error::compile(format!(
					"listener {addrs:?}: SNI {sni_key:?} mapped to two different `tls.managed` blocks"
				)));
			}
		}
		return Ok(());
	}

	let normalised_sni = tls.sni.as_deref().map(str::to_ascii_lowercase);
	let normalised = crate::rule::TlsConfig {
		sni: normalised_sni.clone(),
		cert_file: tls.cert_file.clone(),
		key_file: tls.key_file.clone(),
		managed: None,
		enable_zero_rtt: tls.enable_zero_rtt,
		client_auth: tls.client_auth.clone(),
	};
	match normalised_sni {
		None => match &spec.default {
			None => spec.default = Some(normalised),
			Some(existing) if existing == &normalised => {}
			Some(existing) => {
				return Err(Error::compile(format!(
					"listener {addrs:?}: more than one default (sni-less) cert — {} vs {} — at most one cert may omit `sni`",
					display_cert_file(existing),
					display_cert_file(&normalised),
				)));
			}
		},
		Some(sni_key) => {
			if spec.managed_snis.contains_key(&sni_key) {
				return Err(Error::compile(format!(
					"listener {addrs:?}: SNI {sni_key:?} declared as both static and managed — pick one source"
				)));
			}
			match spec.sni_certs.get(&sni_key) {
				None => {
					spec.sni_certs.insert(sni_key, normalised);
				}
				Some(existing) if existing == &normalised => {}
				Some(existing) => {
					return Err(Error::compile(format!(
						"listener {addrs:?}: SNI {sni_key:?} mapped to two different certs — {} vs {}",
						display_cert_file(existing),
						display_cert_file(&normalised),
					)));
				}
			}
		}
	}
	Ok(())
}

fn resolve_listener_tls(
	addrs: &[SocketAddr],
	rules: &[&AnalyzedRule],
) -> Result<Option<crate::rule::ListenerTlsSpec>, Error> {
	let any_l4 = rules.iter().any(|r| r.posture == Posture::L4);
	let any_tls = rules.iter().any(|r| r.raw.tls.is_some());
	if any_l4 && any_tls {
		return Err(Error::compile(format!(
			"listener {addrs:?}: TLS termination is L7-only — remove `tls` or change the terminator to an L7 type (http_proxy / static / websocket / redirect_https)"
		)));
	}

	let mut spec = crate::rule::ListenerTlsSpec {
		default: None,
		sni_certs: BTreeMap::new(),
		managed_snis: BTreeMap::new(),
		client_auth: crate::rule::ClientAuthSpec::None,
		enable_zero_rtt: false,
	};
	for rule in rules {
		let Some(tls) = rule.raw.tls.as_ref() else { continue };
		// `analyze::analyze_rule` has already enforced
		// `TlsConfig::validate` per spec/acme.md § _Compile-time checks_,
		// so by the time lower iterates each `tls` block here the
		// invariants (exactly one cert source, managed-required SNI,
		// etc.) hold. Branch on cert source to route the rule into
		// the right per-listener bucket.
		route_tls_config_into_spec(addrs, tls, &mut spec)?;
	}

	// Aggregate per-rule `tls.client_auth` into one listener-level
	// `ClientAuthSpec`. Per `08-tls.md` § _Client certificate
	// verification_, mTLS is per-listener: rules on the same listener
	// must agree on `mode` AND `trust_store`. The first rule's spec
	// (after structural validation) becomes the listener's policy;
	// subsequent rules must produce the same value.
	let mut resolved: Option<crate::rule::ClientAuthSpec> = None;
	for rule in rules {
		let Some(tls) = rule.raw.tls.as_ref() else { continue };
		let Some(ca) = tls.client_auth.as_ref() else { continue };
		let candidate = compile_client_auth(addrs, ca)?;
		match &resolved {
			None => resolved = Some(candidate),
			Some(existing) if existing == &candidate => {}
			Some(_) => {
				return Err(Error::compile(format!(
					"listener {addrs:?}: rules disagree on `client_auth` — mTLS is per-listener; every rule on the same address must agree on mode and trust_store"
				)));
			}
		}
	}
	if let Some(ca) = resolved {
		spec.client_auth = ca;
	}

	// Aggregate per-rule `tls.enable_zero_rtt` into the listener-level
	// flag. Mirrors the `client_auth` pattern above: rules on the same
	// listener must agree, since the listener owns one `ServerConfig`
	// (and thus one `max_early_data_size`). Per `08-tls.md` § _TLS 1.3
	// 0-RTT (early data)_.
	let mut zero_rtt_resolved: Option<bool> = None;
	for rule in rules {
		let Some(tls) = rule.raw.tls.as_ref() else { continue };
		match zero_rtt_resolved {
			None => zero_rtt_resolved = Some(tls.enable_zero_rtt),
			Some(existing) if existing == tls.enable_zero_rtt => {}
			Some(_) => {
				return Err(Error::compile(format!(
					"listener {addrs:?}: rules disagree on `tls.enable_zero_rtt` — 0-RTT is a listener-level setting (the listener has one TLS config); every rule on the same address must agree"
				)));
			}
		}
	}
	if let Some(z) = zero_rtt_resolved {
		spec.enable_zero_rtt = z;
	}

	if spec.is_empty() { Ok(None) } else { Ok(Some(spec)) }
}

/// Render a `TlsConfig`'s `cert_file` for use in a compile diagnostic.
/// Static configs always have a path post-validation; the `<managed>`
/// fallback arm is for diagnostic robustness if the validation
/// invariant is ever violated upstream.
fn display_cert_file(tls: &crate::rule::TlsConfig) -> String {
	match &tls.cert_file {
		Some(p) => p.display().to_string(),
		None => "<managed>".to_owned(),
	}
}

/// Cross-listener compile-time warning per `spec/acme.md`
/// § _HTTP-01 § Compile-time checks_: when any rule asks for an
/// HTTP-01 ACME cert but no plaintext `:80` listener exists in the
/// compiled config, the operator should know `vaned` will try to
/// auto-bind `:80` at runtime — and that the bind may fail
/// (`EACCES` without `CAP_NET_BIND_SERVICE`, `EADDRINUSE` if
/// something else owns the port).
///
/// Emitted via `tracing::warn!` rather than `Result::Err` because
/// the auto-bind path makes this a soft signal, not a compile
/// failure. Stage 3 may surface it through the dry-run annotation
/// channel for richer UX.
fn warn_missing_plaintext_port_80_for_http01(
	listener_tls: &std::collections::BTreeMap<SocketAddr, crate::rule::ListenerTlsSpec>,
	listener_kinds: &std::collections::BTreeMap<SocketAddr, ListenerKind>,
) {
	let any_http01 = listener_tls.values().any(|spec| {
		spec.managed_snis.values().any(|m| matches!(m.challenge, crate::rule::ChallengeKind::Http01))
	});
	if !any_http01 {
		return;
	}
	let has_plaintext_80 = listener_kinds.iter().any(|(addr, kind)| {
		addr.port() == 80
			&& matches!(kind, ListenerKind::Http | ListenerKind::Auto)
			&& !listener_tls.contains_key(addr)
	});
	if !has_plaintext_80 {
		tracing::warn!(
			target: "vane::compile::acme",
			"http-01 challenge declared but no plaintext :80 listener exists; \
			 vaned will auto-bind :80 at runtime — the bind may fail without \
			 CAP_NET_BIND_SERVICE or if the port is already in use",
		);
	}
}

/// Per-listener structural validation of the rule-level
/// `allow_zero_rtt` field and its interaction with the listener's
/// `tls.enable_zero_rtt`. Mirrors the constraint table in
/// `08-tls.md` § _TLS 1.3 0-RTT_ § _Compile-time constraints_:
///
/// - On a TLS-L7 listener (`resolved_tls.is_some()`) every rule must
///   set `allow_zero_rtt` to `Some(_)`.
/// - On a plaintext / L4 listener no rule may set `allow_zero_rtt`.
/// - `allow_zero_rtt: true` is rejected when the listener resolved to
///   `enable_zero_rtt: false`.
/// - `allow_zero_rtt: true` requires the rule's match predicate to
///   constrain `http.method` to a subset of {GET, HEAD, OPTIONS}.
fn validate_zero_rtt_for_listener(
	addrs: &[SocketAddr],
	rules: &[&AnalyzedRule],
	resolved_tls: Option<&crate::rule::ListenerTlsSpec>,
) -> Result<(), Error> {
	let tls_l7 = resolved_tls.is_some();
	let listener_enable_zero_rtt = resolved_tls.is_some_and(|s| s.enable_zero_rtt);

	for rule in rules {
		match (tls_l7, rule.raw.allow_zero_rtt) {
			(true, None) => {
				return Err(Error::compile(format!(
					"rule {:?} on TLS-terminating listener {addrs:?}: `allow_zero_rtt` is required (no implicit default) — set it to `true` or `false`",
					rule.raw.name
				)));
			}
			(false, Some(_)) => {
				return Err(Error::compile(format!(
					"rule {:?} on listener {addrs:?}: `allow_zero_rtt` is meaningful only on L7 rules whose listener is TLS-terminating — drop the field",
					rule.raw.name
				)));
			}
			(true, Some(true)) => {
				if !listener_enable_zero_rtt {
					return Err(Error::compile(format!(
						"allow_zero_rtt: true on rule {:?} but listener {addrs:?} has enable_zero_rtt: false",
						rule.raw.name
					)));
				}
				if !predicate_constrains_method_to_idempotent(rule.raw.match_predicate.as_ref()) {
					return Err(Error::compile(format!(
						"allow_zero_rtt: true on rule {:?} requires a method constraint restricted to GET / HEAD / OPTIONS",
						rule.raw.name
					)));
				}
			}
			(true, Some(false)) | (false, None) => {}
		}
	}
	Ok(())
}

/// Walk a rule's match predicate and return `true` iff it structurally
/// restricts `http.method` to a subset of the idempotent set
/// {GET, HEAD, OPTIONS}. Implements the compile-time gate described in
/// `08-tls.md` § _TLS 1.3 0-RTT_ § _Compile-time constraints_.
///
/// Recursive rules:
/// - `Check{ HttpMethod, equals "GET"|"HEAD"|"OPTIONS" }` → idempotent
/// - `Check{ HttpMethod, in [list] }` where every element is one of
///   GET/HEAD/OPTIONS → idempotent
/// - `AllOf` → idempotent iff at least one child is idempotent (the
///   conjunction of restrictions narrows the allowed set further)
/// - `AnyOf` → idempotent iff EVERY alternative is independently
///   idempotent (otherwise the disjunction admits a non-idempotent
///   method)
/// - `Not` and other predicates → not idempotent (cannot reason about
///   the negation's allowed-method set structurally)
///
/// `None` (no match predicate at all) → not idempotent: the rule
/// matches every method, including POST.
fn predicate_constrains_method_to_idempotent(pred: Option<&Predicate>) -> bool {
	let Some(pred) = pred else {
		return false;
	};
	match pred {
		Predicate::Check(c) => check_is_idempotent_method(c),
		Predicate::AllOf(a) => {
			a.all_of.iter().any(|child| predicate_constrains_method_to_idempotent(Some(child)))
		}
		Predicate::AnyOf(a) => {
			!a.any_of.is_empty()
				&& a.any_of.iter().all(|child| predicate_constrains_method_to_idempotent(Some(child)))
		}
		Predicate::Not(_) => false,
	}
}

fn check_is_idempotent_method(c: &crate::predicate::CheckMap) -> bool {
	use crate::predicate::{Operator, Value as PredValue};
	if !matches!(c.path, FieldPath::HttpMethod) {
		return false;
	}
	match &c.op {
		Operator::Equals(PredValue::Str(s)) => is_idempotent_method(s),
		Operator::In(values) => {
			!values.is_empty()
				&& values.iter().all(|v| matches!(v, PredValue::Str(s) if is_idempotent_method(s)))
		}
		_ => false,
	}
}

fn is_idempotent_method(method: &str) -> bool {
	matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// Validate one rule's `client_auth` block and produce the
/// listener-level `ClientAuthSpec` it implies. Compile errors surface
/// every structural omission listed in `08-tls.md` § _Client
/// certificate verification_'s schema table.
fn compile_client_auth(
	addrs: &[SocketAddr],
	ca: &crate::rule::ClientAuthConfig,
) -> Result<crate::rule::ClientAuthSpec, Error> {
	use crate::rule::{ClientAuthMode, ClientAuthSpec};
	match ca.mode {
		ClientAuthMode::None => {
			if ca.trust_store.is_some() {
				return Err(Error::compile(format!(
					"listener {addrs:?}: `client_auth.mode = \"none\"` cannot carry a `trust_store` — drop the trust_store or change the mode"
				)));
			}
			Ok(ClientAuthSpec::None)
		}
		ClientAuthMode::Request | ClientAuthMode::Require => {
			let Some(ts) = ca.trust_store.clone() else {
				return Err(Error::compile(format!(
					"listener {addrs:?}: `client_auth.mode = \"{}\"` requires a `trust_store`",
					match ca.mode {
						ClientAuthMode::Request => "request",
						ClientAuthMode::Require => "require",
						ClientAuthMode::None => unreachable!(),
					}
				)));
			};
			if ts.ca_paths.is_empty() && ts.ca_dir.is_none() {
				return Err(Error::compile(format!(
					"listener {addrs:?}: `trust_store` requires at least one of `ca_paths` or `ca_dir`"
				)));
			}
			Ok(match ca.mode {
				ClientAuthMode::Request => ClientAuthSpec::Request { trust_store: ts },
				ClientAuthMode::Require => ClientAuthSpec::Require { trust_store: ts },
				ClientAuthMode::None => unreachable!(),
			})
		}
	}
}

fn group_by_listener<'a>(rules: &'a [AnalyzedRule]) -> Result<Vec<ListenerGroup<'a>>, Error> {
	// Keyed by `(transport, sorted addrs)` so rules whose listen
	// strings expand to the same address set under the same transport
	// share one group; mismatched transports (e.g. `tcp:443` vs
	// `udp:443`) form distinct listeners. Same-rule mixed prefixes
	// across one rule's `listen` array (e.g. `["tcp:80", "udp:443"]`)
	// are explicitly rejected — the lower pipeline carries one
	// transport per rule entry today; multi-transport rules are a
	// future enhancement.
	let mut groups: HashMap<(Transport, Vec<SocketAddr>), Vec<&'a AnalyzedRule>> = HashMap::new();
	for rule in rules {
		let mut transport: Option<Transport> = None;
		let mut addrs: Vec<SocketAddr> = Vec::new();
		for spec in &rule.raw.listen {
			let (t, more) = parse_listen(spec)?;
			match transport {
				None => transport = Some(t),
				Some(existing) if existing == t => {}
				Some(existing) => {
					return Err(Error::compile(format!(
						"rule {:?}: `listen` mixes transports {:?} and {:?} in one rule — split into separate rules",
						rule.raw.name, existing, t,
					)));
				}
			}
			addrs.extend(more);
		}
		addrs.sort();
		addrs.dedup();
		// Defensive: a rule with no listen entries can't produce an
		// `Option<Transport>`, but the parser elsewhere already
		// rejects empty `listen` arrays — fall back to TCP so this
		// branch is unreachable in practice.
		let transport = transport.unwrap_or(Transport::Tcp);
		groups.entry((transport, addrs)).or_default().push(rule);
	}
	let mut out: Vec<ListenerGroup<'_>> =
		groups.into_iter().map(|((transport, addrs), rules)| (transport, addrs, rules)).collect();
	out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
	Ok(out)
}

/// Parse one `ListenSpec` entry into its declared `(transport, addrs)`
/// pair per `09-config.md` § _`ListenSpec` grammar_. The optional
/// `tcp:` / `udp:` prefix declares the listener's wire transport;
/// bare entries default to TCP for backwards compatibility (the spec
/// table's `_(none)_` row).
///
/// Address parsing is unchanged from the pre-prefix grammar — the
/// remainder after the prefix is parsed by the same dual-stack /
/// IPv4-only / IPv6-only / specific-bind ladder.
///
/// # Errors
///
/// - Wildcard port `:0` / `*:0` (with or without prefix) → rejected.
/// - Bare malformed prefix `udpx:443` / `TCP:443` (uppercase) →
///   falls through to the address parser, which rejects the leading
///   non-IP token.
/// - Inner `udp::443` strips the `udp:` and parses `:443` as a bare
///   dual-stack port (the inner `:` is part of the address).
fn parse_listen(spec: &str) -> Result<(Transport, Vec<SocketAddr>), Error> {
	let s = spec.trim();
	let (transport, rest, prefix_seen) = if let Some(rest) = s.strip_prefix("tcp:") {
		(Transport::Tcp, rest, true)
	} else if let Some(rest) = s.strip_prefix("udp:") {
		(Transport::Udp, rest, true)
	} else {
		(Transport::Tcp, s, false)
	};

	// Per `09-config.md` § _ListenSpec grammar_'s composition table,
	// `tcp:443` / `udp:443` are valid (the spec example concatenates
	// the prefix with a bare port form). After stripping the prefix
	// the remainder is a naked digit string; treat that as the
	// existing `:443` dual-stack form. Bare-port without prefix
	// (`443`) keeps the existing rejection — it isn't in the
	// address-forms table.
	let owned: String;
	let parse_target: &str =
		if prefix_seen && !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
			owned = format!(":{rest}");
			&owned
		} else {
			rest
		};

	let addrs = parse_listen_address(parse_target, spec)?;
	Ok((transport, addrs))
}

/// Address-portion parser. Split out from [`parse_listen`] so the
/// transport-prefix path can reuse it without re-stripping; tests on
/// the address grammar exercise this directly.
fn parse_listen_address(rest: &str, original: &str) -> Result<Vec<SocketAddr>, Error> {
	// Wildcard-port rejection per 09-config.md.
	if rest == ":0" || rest == "*:0" {
		return Err(Error::compile(format!("wildcard port rejected: {original:?}")));
	}
	// Dual-stack shorthand `:443` or `*:443` → v4 + v6.
	if let Some(port_str) = rest.strip_prefix(':').or_else(|| rest.strip_prefix("*:")) {
		let port = u16::from_str(port_str)
			.map_err(|e| Error::compile(format!("bad port in {original:?}: {e}")))?;
		return Ok(vec![
			SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
			SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port),
		]);
	}
	SocketAddr::from_str(rest)
		.map(|a| vec![a])
		.map_err(|e| Error::compile(format!("bad listen spec {original:?}: {e}")))
}

fn compile_operator(
	op: &Operator,
	path: &FieldPath,
	source: &SourceInfo,
) -> Result<CompiledOperator, Error> {
	// Spec 18 § _Operator × value type compatibility_: reject any
	// (path, op) pair that the matrix marks `—`. The (path, op) pair
	// uniquely picks an OperatorFamily row and the field's value-type
	// column, so a single matrix lookup covers every illegal case
	// before we touch the operator-specific coerce path.
	let family = op.family();
	let vt = path.value_type();
	if !family.accepts(vt) {
		return Err(Error::compile(format!(
			"{}operator `{}` cannot apply to field `{}` (expected {}, got {})",
			source_prefix(source),
			op.name(),
			path.display_name(),
			family.family_expectation(),
			vt.name(),
		)));
	}

	Ok(match op {
		Operator::Equals(v) => CompiledOperator::Equals(coerce_value(v, path, op.name(), source)?),
		Operator::NotEquals(v) => {
			CompiledOperator::NotEquals(coerce_value(v, path, op.name(), source)?)
		}
		Operator::Contains(v) => {
			CompiledOperator::Contains(value_to_bytes(v, path, op.name(), source)?)
		}
		Operator::NotContains(v) => {
			CompiledOperator::NotContains(value_to_bytes(v, path, op.name(), source)?)
		}
		Operator::Prefix(v) => CompiledOperator::Prefix(value_to_bytes(v, path, op.name(), source)?),
		Operator::Suffix(v) => CompiledOperator::Suffix(value_to_bytes(v, path, op.name(), source)?),
		Operator::Matches(pat) => {
			CompiledOperator::Matches(fancy_regex::Regex::new(pat).map_err(|e| {
				Error::compile(format!(
					"{}invalid regex in `matches` operator on field `{}`: {e}",
					source_prefix(source),
					path.display_name(),
				))
			})?)
		}
		Operator::In(vs) => {
			let mut out = Vec::with_capacity(vs.len());
			for v in vs {
				out.push(coerce_value(v, path, op.name(), source)?);
			}
			CompiledOperator::In(out)
		}
		Operator::NotIn(vs) => {
			let mut out = Vec::with_capacity(vs.len());
			for v in vs {
				out.push(coerce_value(v, path, op.name(), source)?);
			}
			CompiledOperator::NotIn(out)
		}
		Operator::Gt(n) => CompiledOperator::Gt(*n),
		Operator::Gte(n) => CompiledOperator::Gte(*n),
		Operator::Lt(n) => CompiledOperator::Lt(*n),
		Operator::Lte(n) => CompiledOperator::Lte(*n),
		Operator::Cidr(s) => CompiledOperator::Cidr(ipnet::IpNet::from_str(s).map_err(|e| {
			Error::compile(format!(
				"{}invalid cidr `{s}` on field `{}`: {e}",
				source_prefix(source),
				path.display_name(),
			))
		})?),
	})
}

fn coerce_value(
	v: &Value,
	path: &FieldPath,
	op_name: &'static str,
	source: &SourceInfo,
) -> Result<CompiledValue, Error> {
	let mismatch = || {
		Error::compile(format!(
			"{}field `{}` ({}) is not compatible with `{op_name}` value {}",
			source_prefix(source),
			path.display_name(),
			path.value_type().name(),
			value_kind(v),
		))
	};
	match path.value_type() {
		FieldValueType::IpAddr => {
			let Value::Str(s) = v else {
				return Err(mismatch());
			};
			IpAddr::from_str(s).map(CompiledValue::Addr).map_err(|e| {
				Error::compile(format!(
					"{}field `{}` expects an ip-address string, got {s:?}: {e}",
					source_prefix(source),
					path.display_name(),
				))
			})
		}
		FieldValueType::Int => {
			let Value::Int(n) = v else {
				return Err(mismatch());
			};
			Ok(CompiledValue::Int(*n))
		}
		FieldValueType::Bytes => {
			// spec/architecture/18-predicate-schema.md § _Value JSON
			// encoding_: bytes-typed fields take a STANDARD base64
			// string. Decoding here keeps the lower-time IR aligned
			// with the dry-run JSON form (which the shadow-enum's
			// de_bytes already round-trips through base64).
			let Value::Str(s) = v else {
				return Err(mismatch());
			};
			let decoded = B64.decode(s.as_bytes()).map_err(|e| {
				Error::compile(format!(
					"{}operator `{op_name}` on field `{}` expected base64 string: {e}",
					source_prefix(source),
					path.display_name(),
				))
			})?;
			Ok(CompiledValue::Bytes(bytes::Bytes::from(decoded)))
		}
		FieldValueType::Str => {
			let Value::Str(s) = v else {
				return Err(mismatch());
			};
			Ok(CompiledValue::Str(Arc::from(s.as_str())))
		}
		FieldValueType::Enum => {
			let Value::Str(s) = v else {
				return Err(mismatch());
			};
			coerce_enum_value(path, s, source)
		}
		FieldValueType::Bool => {
			let Value::Bool(b) = v else {
				return Err(mismatch());
			};
			Ok(CompiledValue::Bool(*b))
		}
		// Vec<Str>: equals/in/etc. are matrix-rejected; the only legal
		// operators are `contains`/`not_contains` whose operand is a
		// single string (semantics: list contains/does-not-contain
		// this exact element). They route through `value_to_bytes`,
		// not this helper, so any path here is a matrix-rejected pair
		// that already errored. We surface it explicitly to avoid a
		// silent fall-through.
		FieldValueType::VecStr => Err(Error::compile(format!(
			"{}field `{}` ({}) cannot be operand-coerced — only `contains` / `not_contains` apply to Vec<Str>",
			source_prefix(source),
			path.display_name(),
			path.value_type().name(),
		))),
	}
}

fn coerce_enum_value(
	path: &FieldPath,
	s: &str,
	source: &SourceInfo,
) -> Result<CompiledValue, Error> {
	let allowed: Option<&[&str]> = match path {
		FieldPath::Transport => Some(&["tcp", "udp"]),
		FieldPath::TlsVersion => Some(&["1.2", "1.3"]),
		// `http.method` is open per spec — any HTTP token is admissible
		// at compile; runtime byte-compares to `Request.method().as_str()`.
		FieldPath::HttpMethod => None,
		_ => unreachable!("non-enum path reached coerce_enum_value: {path:?}"),
	};
	if let Some(values) = allowed
		&& !values.contains(&s)
	{
		return Err(Error::compile(format!(
			"{}field `{}` accepts {:?}, got {s:?}",
			source_prefix(source),
			path.display_name(),
			values,
		)));
	}
	Ok(CompiledValue::Str(Arc::from(s)))
}

fn value_to_bytes(
	v: &Value,
	path: &FieldPath,
	op_name: &'static str,
	source: &SourceInfo,
) -> Result<bytes::Bytes, Error> {
	// spec 18 § _Value JSON encoding_ keys the literal form off the
	// FIELD's value type, not the operator: String-valued fields take
	// a verbatim JSON string; Bytes-valued fields take a STANDARD
	// base64 string. contains / not_contains / prefix / suffix all
	// route through this helper but the encoding still tracks the
	// field — `prefix` on `http.uri.path` (Str) keeps the raw bytes
	// of the literal, while `contains` on `http.body` (Bytes)
	// base64-decodes.
	match v {
		Value::Str(s) => match path.value_type() {
			FieldValueType::Bytes => B64.decode(s.as_bytes()).map(bytes::Bytes::from).map_err(|e| {
				Error::compile(format!(
					"{}operator `{op_name}` on field `{}` expected base64 string: {e}",
					source_prefix(source),
					path.display_name(),
				))
			}),
			_ => Ok(bytes::Bytes::copy_from_slice(s.as_bytes())),
		},
		Value::Int(_) | Value::Bool(_) => Err(Error::compile(format!(
			"{}operator `{op_name}` on field `{}` expects a string value, got {}",
			source_prefix(source),
			path.display_name(),
			value_kind(v),
		))),
	}
}

fn value_kind(v: &Value) -> &'static str {
	match v {
		Value::Str(_) => "Str",
		Value::Int(_) => "Int",
		Value::Bool(_) => "Bool",
	}
}

fn source_prefix(source: &SourceInfo) -> String {
	if source.file.as_os_str().is_empty() {
		String::new()
	} else {
		format!("{}:{}: ", source.file.display(), source.line)
	}
}

fn canonical_json(v: &serde_json::Value) -> String {
	use serde_json::Value as V;
	match v {
		V::Null => "null".to_string(),
		V::Bool(b) => b.to_string(),
		V::Number(n) => n.to_string(),
		V::String(s) => serde_json::to_string(s).unwrap_or_else(|_| s.clone()),
		V::Array(xs) => {
			let parts: Vec<String> = xs.iter().map(canonical_json).collect();
			format!("[{}]", parts.join(","))
		}
		V::Object(xs) => {
			let mut keys: Vec<&String> = xs.keys().collect();
			keys.sort();
			let parts: Vec<String> = keys
				.iter()
				.map(|k| {
					format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), canonical_json(&xs[*k]))
				})
				.collect();
			format!("{{{}}}", parts.join(","))
		}
	}
}

fn hash_rules(rules: &[AnalyzedRule]) -> [u8; 32] {
	let mut hasher = Sha256::new();
	for rule in rules {
		hasher.update(rule.raw.name.as_bytes());
		hasher.update(b"|");
		for spec in &rule.raw.listen {
			hasher.update(spec.as_bytes());
			hasher.update(b",");
		}
		hasher.update(b"|");
		hasher.update(canonical_json(&rule.raw.terminate.args).as_bytes());
		hasher.update(b"||");
	}
	let _ = PathBuf::new;
	hasher.finalize().into()
}

#[cfg(test)]
mod compat_tests {
	//! Per-cell coverage of spec 18 § _Operator × value type
	//! compatibility_. Each illegal cell (marked `—` in the matrix) gets
	//! at least one rejected sample, and the diagnostic carries the rule
	//! file + line. Legal cells are exercised indirectly by the runtime
	//! dispatch tests in `crate::predicate`.
	use std::path::PathBuf;
	use std::sync::Arc;

	use super::{SourceInfo, compile_operator};
	use crate::predicate::{FieldPath, Operator, Value};

	fn src() -> SourceInfo {
		SourceInfo { file: PathBuf::from("rules/30-api.json"), line: 14 }
	}

	fn assert_rejected_with_source(err: &crate::error::Error) {
		let msg = err.to_string();
		assert!(msg.contains("rules/30-api.json:14"), "error must carry rule source: {msg}");
	}

	#[test]
	fn gt_on_bytes_field_rejected() {
		let err = compile_operator(&Operator::Gt(100), &FieldPath::HttpBody, &src())
			.expect_err("gt on http.body must reject");
		let msg = err.to_string();
		assert!(msg.contains("`gt`"), "{msg}");
		assert!(msg.contains("http.body"), "{msg}");
		assert!(msg.contains("expected numeric"), "{msg}");
		assert_rejected_with_source(&err);
	}

	#[test]
	fn cidr_on_string_field_rejected() {
		let err =
			compile_operator(&Operator::Cidr("10.0.0.0/8".to_string()), &FieldPath::HttpUriPath, &src())
				.expect_err("cidr on http.uri.path must reject");
		let msg = err.to_string();
		assert!(msg.contains("`cidr`"), "{msg}");
		assert!(msg.contains("http.uri.path"), "{msg}");
		assert!(msg.contains("expected IpAddr"), "{msg}");
		assert_rejected_with_source(&err);
	}

	#[test]
	fn matches_on_bytes_field_rejected() {
		let err = compile_operator(&Operator::Matches("^a".to_string()), &FieldPath::TlsAlpn, &src())
			.expect_err("matches on tls.alpn must reject");
		let msg = err.to_string();
		assert!(msg.contains("`matches`"), "{msg}");
		assert!(msg.contains("expected Str"), "{msg}");
	}

	#[test]
	fn matches_on_int_field_rejected() {
		let err =
			compile_operator(&Operator::Matches("^1".to_string()), &FieldPath::RemotePort, &src())
				.expect_err("matches on remote.port must reject");
		let msg = err.to_string();
		assert!(msg.contains("`matches`"), "{msg}");
		assert!(msg.contains("expected Str"), "{msg}");
	}

	#[test]
	fn contains_on_int_field_rejected() {
		let err = compile_operator(&Operator::Contains(Value::Int(1)), &FieldPath::RemotePort, &src())
			.expect_err("contains on remote.port must reject");
		let msg = err.to_string();
		assert!(msg.contains("`contains`"), "{msg}");
		assert!(msg.contains("Str, Bytes, or Vec<Str>"), "{msg}");
	}

	#[test]
	fn prefix_on_ip_field_rejected() {
		let err = compile_operator(
			&Operator::Prefix(Value::Str("10.".to_string())),
			&FieldPath::RemoteIp,
			&src(),
		)
		.expect_err("prefix on remote.ip must reject");
		let msg = err.to_string();
		assert!(msg.contains("`prefix`"), "{msg}");
		assert!(msg.contains("Str or Bytes"), "{msg}");
	}

	#[test]
	fn suffix_on_enum_field_rejected() {
		let err = compile_operator(
			&Operator::Suffix(Value::Str("p".to_string())),
			&FieldPath::Transport,
			&src(),
		)
		.expect_err("suffix on transport must reject");
		let msg = err.to_string();
		assert!(msg.contains("`suffix`"), "{msg}");
	}

	#[test]
	fn gt_on_str_field_rejected() {
		let err = compile_operator(&Operator::Gt(0), &FieldPath::TlsSni, &src())
			.expect_err("gt on tls.sni must reject");
		assert!(err.to_string().contains("expected numeric"));
	}

	#[test]
	fn cidr_on_int_field_rejected() {
		let err =
			compile_operator(&Operator::Cidr("10.0.0.0/8".to_string()), &FieldPath::RemotePort, &src())
				.expect_err("cidr on remote.port must reject");
		assert!(err.to_string().contains("expected IpAddr"));
	}

	#[test]
	fn cidr_on_enum_field_rejected() {
		let err =
			compile_operator(&Operator::Cidr("0.0.0.0/0".to_string()), &FieldPath::HttpMethod, &src())
				.expect_err("cidr on http.method must reject");
		assert!(err.to_string().contains("expected IpAddr"));
	}

	#[test]
	fn cidr_on_bytes_field_rejected() {
		let err =
			compile_operator(&Operator::Cidr("10.0.0.0/8".to_string()), &FieldPath::TlsAlpn, &src())
				.expect_err("cidr on tls.alpn must reject");
		assert!(err.to_string().contains("expected IpAddr"));
	}

	#[test]
	fn substring_on_ip_field_rejected() {
		let err = compile_operator(
			&Operator::Contains(Value::Str("10.".to_string())),
			&FieldPath::RemoteIp,
			&src(),
		)
		.expect_err("contains on remote.ip must reject");
		assert!(err.to_string().contains("`contains`"));
		assert!(err.to_string().contains("Str, Bytes, or Vec<Str>"));
	}

	#[test]
	fn substring_on_enum_field_rejected() {
		let err = compile_operator(
			&Operator::NotContains(Value::Str("p".to_string())),
			&FieldPath::Transport,
			&src(),
		)
		.expect_err("not_contains on transport must reject");
		assert!(err.to_string().contains("`not_contains`"));
	}

	#[test]
	fn prefix_suffix_on_int_field_rejected() {
		let err = compile_operator(
			&Operator::Prefix(Value::Str("80".to_string())),
			&FieldPath::RemotePort,
			&src(),
		)
		.expect_err("prefix on remote.port must reject");
		assert!(err.to_string().contains("`prefix`"));
		assert!(err.to_string().contains("Str or Bytes"));
	}

	#[test]
	fn matches_on_ip_field_rejected() {
		let err = compile_operator(&Operator::Matches("^10".to_string()), &FieldPath::RemoteIp, &src())
			.expect_err("matches on remote.ip must reject");
		let msg = err.to_string();
		assert!(msg.contains("`matches`"), "{msg}");
		assert!(msg.contains("expected Str"), "{msg}");
	}

	#[test]
	fn matches_on_enum_field_rejected() {
		let err = compile_operator(&Operator::Matches("^t".to_string()), &FieldPath::Transport, &src())
			.expect_err("matches on transport must reject");
		assert!(err.to_string().contains("expected Str"));
	}

	#[test]
	fn numeric_cmp_on_ip_field_rejected() {
		let err = compile_operator(&Operator::Lt(0), &FieldPath::RemoteIp, &src())
			.expect_err("lt on remote.ip must reject");
		assert!(err.to_string().contains("expected numeric"));
	}

	#[test]
	fn numeric_cmp_on_enum_field_rejected() {
		let err = compile_operator(&Operator::Gte(0), &FieldPath::TlsVersion, &src())
			.expect_err("gte on tls.version must reject");
		assert!(err.to_string().contains("expected numeric"));
	}

	#[test]
	fn invalid_regex_carries_source_and_field() {
		let err =
			compile_operator(&Operator::Matches("[".to_string()), &FieldPath::HttpUriPath, &src())
				.expect_err("unbalanced [ must reject");
		let msg = err.to_string();
		assert!(msg.contains("rules/30-api.json:14"), "{msg}");
		assert!(msg.contains("`matches`"), "{msg}");
		assert!(msg.contains("http.uri.path"), "{msg}");
	}

	#[test]
	fn transport_enum_rejects_unknown_literal() {
		let err = compile_operator(
			&Operator::Equals(Value::Str("ftp".to_string())),
			&FieldPath::Transport,
			&src(),
		)
		.expect_err("transport == \"ftp\" must reject");
		let msg = err.to_string();
		assert!(msg.contains("transport"), "{msg}");
		assert!(msg.contains("\"ftp\""), "{msg}");
	}

	#[test]
	fn transport_enum_accepts_known_literals() {
		for v in ["tcp", "udp"] {
			compile_operator(&Operator::Equals(Value::Str(v.to_string())), &FieldPath::Transport, &src())
				.unwrap_or_else(|e| panic!("transport == {v:?} must compile: {e}"));
		}
	}

	#[test]
	fn tls_version_enum_rejects_unknown_literal() {
		let err = compile_operator(
			&Operator::Equals(Value::Str("0.9".to_string())),
			&FieldPath::TlsVersion,
			&src(),
		)
		.expect_err("tls.version == \"0.9\" must reject");
		assert!(err.to_string().contains("tls.version"));
	}

	#[test]
	fn http_method_enum_accepts_any_string() {
		// Spec leaves http.method as an open enum — any Str literal
		// compiles, the runtime byte-compares to Request::method().as_str().
		compile_operator(
			&Operator::Equals(Value::Str("CONNECT".to_string())),
			&FieldPath::HttpMethod,
			&src(),
		)
		.expect("http.method == CONNECT must compile");
	}

	#[test]
	fn equals_int_value_on_string_field_rejected() {
		// equals/in are matrix-legal on every column, but the value-type
		// coerce still enforces shape: an Int literal on a Str-typed
		// field can't be coerced.
		let err = compile_operator(&Operator::Equals(Value::Int(1)), &FieldPath::TlsSni, &src())
			.expect_err("equals(int) on str field must reject");
		let msg = err.to_string();
		assert!(msg.contains("tls.sni"), "{msg}");
		assert!(msg.contains("Str"), "{msg}");
	}

	#[test]
	fn in_list_with_mixed_types_rejected_on_int_field() {
		let err = compile_operator(
			&Operator::In(vec![Value::Int(1), Value::Str("x".to_string())]),
			&FieldPath::RemotePort,
			&src(),
		)
		.expect_err("in([int,str]) on int field must reject");
		assert!(err.to_string().contains("remote.port"));
	}

	#[test]
	fn empty_source_info_omits_prefix() {
		// SourceInfo::default() carries an empty PathBuf; the prefix
		// helper must collapse to a clean message rather than ":0:".
		let empty = SourceInfo::default();
		let err = compile_operator(&Operator::Gt(100), &FieldPath::HttpBody, &empty)
			.expect_err("gt on http.body must reject");
		let msg = err.to_string();
		assert!(!msg.contains(":0:"), "default source must not leak `:0:` prefix: {msg}");
	}

	#[test]
	fn arc_compiled_for_str_field_uses_string_arc() {
		// Quick sanity that legal Str-field compile path produces an
		// Arc<str> CompiledValue::Str and not a raw String somewhere.
		let op =
			compile_operator(&Operator::Equals(Value::Str("x".to_string())), &FieldPath::TlsSni, &src())
				.expect("legal equals/str compiles");
		match op {
			crate::predicate::CompiledOperator::Equals(crate::predicate::CompiledValue::Str(arc)) => {
				let _: Arc<str> = arc;
			}
			other => panic!("unexpected compiled op: {other:?}"),
		}
	}

	// ── spec 18 § _Value JSON encoding_: bytes-typed literal = STANDARD base64 ──

	/// `{ "http.body": { "contains": "aGVsbG8=" } }` is the spec example
	/// for a Bytes-valued contains operator. Compile must decode the
	/// base64 into the literal bytes b"hello" so the runtime byte-
	/// comparison sees the user's intent.
	#[test]
	fn bytes_literal_decoded_as_base64_for_contains_on_http_body() {
		let op = compile_operator(
			&Operator::Contains(Value::Str("aGVsbG8=".to_string())),
			&FieldPath::HttpBody,
			&src(),
		)
		.expect("base64 contains compiles");
		match op {
			crate::predicate::CompiledOperator::Contains(b) => {
				assert_eq!(b.as_ref(), b"hello", "base64 'aGVsbG8=' must decode to 'hello'");
			}
			other => panic!("expected Contains, got {other:?}"),
		}
	}

	/// Same rule for `equals` on a Bytes-typed field — `coerce_value`
	/// emits `CompiledValue::Bytes` after base64 decoding.
	#[test]
	fn bytes_literal_decoded_as_base64_for_equals_on_tls_alpn() {
		let op = compile_operator(
			&Operator::Equals(Value::Str("aDI=".to_string())),
			&FieldPath::TlsAlpn,
			&src(),
		)
		.expect("base64 equals compiles");
		match op {
			crate::predicate::CompiledOperator::Equals(crate::predicate::CompiledValue::Bytes(b)) => {
				assert_eq!(b.as_ref(), b"h2", "base64 'aDI=' must decode to 'h2'");
			}
			other => panic!("expected Equals(Bytes(\"h2\")), got {other:?}"),
		}
	}

	/// `prefix` / `suffix` on a Bytes-typed field route through
	/// `value_to_bytes` and must base64-decode the literal. On
	/// Str-typed fields the literal stays verbatim — covered by
	/// [`str_field_prefix_suffix_keeps_raw_bytes`] below.
	#[test]
	fn bytes_field_prefix_suffix_decodes_base64() {
		// Peek (Bytes) — TLS ClientHello prefix bytes 0x16 0x03 → "FgM=" in base64.
		let prefix =
			compile_operator(&Operator::Prefix(Value::Str("FgM=".to_string())), &FieldPath::Peek, &src())
				.expect("peek prefix compiles");
		match prefix {
			crate::predicate::CompiledOperator::Prefix(b) => assert_eq!(b.as_ref(), &[0x16, 0x03]),
			other => panic!("expected Prefix, got {other:?}"),
		}

		// HttpBody (Bytes) — base64 of literal bytes b"END".
		let suffix = compile_operator(
			&Operator::Suffix(Value::Str("RU5E".to_string())),
			&FieldPath::HttpBody,
			&src(),
		)
		.expect("body suffix compiles");
		match suffix {
			crate::predicate::CompiledOperator::Suffix(b) => assert_eq!(b.as_ref(), b"END"),
			other => panic!("expected Suffix, got {other:?}"),
		}
	}

	/// String-valued fields keep the raw literal bytes per spec
	/// 18 § _Value JSON encoding_ — base64 only applies when the
	/// FIELD is Bytes-valued, not when the operator produces bytes.
	#[test]
	fn str_field_prefix_suffix_keeps_raw_bytes() {
		let prefix = compile_operator(
			&Operator::Prefix(Value::Str("/api".to_string())),
			&FieldPath::HttpUriPath,
			&src(),
		)
		.expect("str-field prefix compiles verbatim");
		match prefix {
			crate::predicate::CompiledOperator::Prefix(b) => assert_eq!(b.as_ref(), b"/api"),
			other => panic!("expected Prefix, got {other:?}"),
		}

		let suffix = compile_operator(
			&Operator::Suffix(Value::Str(".json".to_string())),
			&FieldPath::HttpUriPath,
			&src(),
		)
		.expect("str-field suffix compiles verbatim");
		match suffix {
			crate::predicate::CompiledOperator::Suffix(b) => assert_eq!(b.as_ref(), b".json"),
			other => panic!("expected Suffix, got {other:?}"),
		}
	}

	/// Non-base64 input rejected at compile, error mentions rule
	/// source so operators can locate the bad rule.
	#[test]
	fn bytes_literal_rejects_non_base64_with_source_prefix() {
		let err = compile_operator(
			&Operator::Contains(Value::Str("###".to_string())),
			&FieldPath::HttpBody,
			&src(),
		)
		.expect_err("non-base64 contains must reject");
		let msg = err.to_string();
		assert!(msg.contains("rules/30-api.json:14"), "error must carry source: {msg}");
		assert!(msg.contains("`contains`"), "{msg}");
		assert!(msg.contains("http.body"), "{msg}");
		assert!(msg.contains("expected base64 string"), "{msg}");
	}

	/// Equals/In on a Bytes-typed field share the same base64 contract;
	/// non-base64 must reject with source prefix.
	#[test]
	fn bytes_literal_equals_rejects_non_base64() {
		let err = compile_operator(
			&Operator::Equals(Value::Str("not-valid-base64!".to_string())),
			&FieldPath::TlsAlpn,
			&src(),
		)
		.expect_err("non-base64 equals must reject");
		let msg = err.to_string();
		assert!(msg.contains("expected base64 string"), "{msg}");
		assert!(msg.contains("tls.alpn"), "{msg}");
	}

	/// End-to-end via parse + lower: spec example object compiles to
	/// a Check whose `CompiledOperator::Contains` carries b"hello".
	#[test]
	fn parse_and_lower_spec_example_decodes_base64_contains() {
		// Round-trip through Predicate::Check just like a real rule
		// would. The spec example is verbatim from
		// spec/architecture/18-predicate-schema.md § _Value JSON encoding_.
		let raw = serde_json::json!({ "http.body": { "contains": "aGVsbG8=" } });
		let pred: crate::predicate::Predicate = serde_json::from_value(raw).expect("parse predicate");
		let check = match pred {
			crate::predicate::Predicate::Check(c) => c,
			other => panic!("expected Check, got {other:?}"),
		};
		let op = compile_operator(&check.op, &check.path, &src()).expect("lower");
		match op {
			crate::predicate::CompiledOperator::Contains(b) => assert_eq!(b.as_ref(), b"hello"),
			other => panic!("expected Contains, got {other:?}"),
		}
	}
}
