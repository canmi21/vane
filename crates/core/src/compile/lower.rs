use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::compile::analyze::{AnalyzedRule, AnalyzedRuleSet, Posture};
use crate::error::Error;
use crate::fetch::{FetchKind, SymbolicFetchRef, Terminator};
use crate::ir::{
	BodySide, FetchId, FlowGraphMeta, MiddlewareId, Node, NodeId, PredicateId, SymbolicFlowGraph,
	TerminatorId,
};
use crate::metadata::{FetchMetadataProvider, MiddlewareMetadataProvider};
use crate::middleware::{MiddlewareKind, SymbolicMiddlewareRef};
use crate::predicate::{
	CompiledOperator, CompiledValue, FieldPath, Operator, Predicate, PredicateInst, Value,
};

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
	for (addrs, rules) in groups {
		let entry = builder.lower_port(&rules, mw_meta, fetch_meta)?;
		for addr in addrs {
			builder.entries.insert(addr, entry);
		}
	}

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
		},
	})
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
		// is the first rule's first node — the default-miss would be dead
		// code, and L4's default-miss cannot be synthesised today anyway.
		let needs_fallback = ordered.iter().any(|r| r.raw.match_predicate.is_some());
		let fallback_miss = if needs_fallback {
			self.synthesize_default_miss(posture)?
		} else {
			// Use a sentinel; it is never read because no Check node will
			// point at it. `NodeId::new(u32::MAX)` would also work but picking
			// the last real fetch-id-less value keeps dumps clean.
			NodeId::new(0)
		};
		let mut current_miss = fallback_miss;
		for rule in ordered.iter().rev() {
			let chain_entry = self.lower_rule(rule, current_miss, mw_meta, fetch_meta)?;
			current_miss = chain_entry;
		}
		Ok(current_miss)
	}

	fn synthesize_default_miss(&mut self, posture: Posture) -> Result<NodeId, Error> {
		match posture {
			Posture::L7 => {
				let fid = self.push_fetch(SymbolicFetchRef {
					kind: FetchKind::HttpSynthesize,
					args: serde_json::json!({ "status": 500, "body": "Internal Server Error" }),
				});
				let tid = self.intern_terminator(Terminator::WriteHttpResponse);
				let term_node = self.push_node(Node::Terminate(tid));
				let fetch_node = self.push_node(Node::Fetch {
					id: fid,
					next_response: Some(term_node),
					next_tunnel: None,
					collect_body_before: None,
				});
				Ok(fetch_node)
			}
			Posture::L4 => Err(Error::compile(
				"L4 listener requires a catch-all rule — no default close terminator yet".to_string(),
			)),
		}
	}

	fn lower_rule(
		&mut self,
		rule: &AnalyzedRule,
		on_miss: NodeId,
		mw_meta: &dyn MiddlewareMetadataProvider,
		fetch_meta: &dyn FetchMetadataProvider,
	) -> Result<NodeId, Error> {
		// Build tail-first so on_* edges point at already-allocated NodeIds.
		let terminator_variant = terminator_for_fetch(rule.raw.terminate.kind);
		let tid = self.intern_terminator(terminator_variant);
		let term_node = self.push_node(Node::Terminate(tid));

		// Fetch node. LazyBuffer response-side flag attaches here if the
		// response track has no earlier reader (S1-22 middleware introduces
		// response-side nodes; for now the fetch is the only candidate).
		let fetch_kind = rule.raw.terminate.kind;
		let fid =
			self.push_fetch(SymbolicFetchRef { kind: fetch_kind, args: rule.raw.terminate.args.clone() });
		let (next_response, next_tunnel) = match fetch_kind {
			FetchKind::HttpProxy | FetchKind::HttpSynthesize => (Some(term_node), None),
			FetchKind::L4Forward => (None, Some(term_node)),
			FetchKind::WebSocketUpgrade => (Some(term_node), Some(term_node)),
		};
		let _ = fetch_meta;
		let fetch_node_idx = self.nodes.len();
		let fetch_node_id = NodeId::new(u32::try_from(fetch_node_idx).expect("node id fits u32"));
		self.nodes.push(Node::Fetch { id: fid, next_response, next_tunnel, collect_body_before: None });

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
			let node = Node::Middleware { id, next: head, on_error: None, collect_body_before: None };
			head = self.push_node(node);
		}

		// Second pass (forward): place LazyBuffer first-reader flags. We walk
		// from the chain's entry (head) forward to fetch, flagging the first
		// node that reads the body on each side.
		let chain_entry_before_upgrade = head;
		let _ = (&mut req_first_reader_seen, &mut resp_first_reader_seen);
		if rule.needs_request_body {
			self.mark_request_reader(chain_entry_before_upgrade, mw_meta)?;
		}
		if rule.needs_response_body {
			// Response side: today the only response-readers are L7Response
			// middleware past Fetch. No such middleware lands in this chunk;
			// spec-correctly, leave the flag off until Fetch-adjacent code
			// can set it post-Fetch in the response sub-chain (S2 scope).
		}

		// Insert Upgrade for L7 posture when any predicate reads L4 state.
		// Simplified: for L7 posture we always emit Upgrade between the L4
		// check (if any) and the post-Upgrade chain head.
		let chain_entry_after_predicate_split;
		if rule.posture == Posture::L7 {
			let l4_pre_upgrade = predicate_is_l4(rule.raw.match_predicate.as_ref());
			if l4_pre_upgrade {
				// Upgrade sits between the L4 check and the L7 chain head.
				let upgrade_id = self.push_node(Node::Upgrade { next: head });
				head = upgrade_id;
				chain_entry_after_predicate_split = head;
			} else {
				// Upgrade sits at the top of the chain, before predicates.
				let upgrade_id = self.push_node(Node::Upgrade { next: head });
				head = upgrade_id;
				chain_entry_after_predicate_split = head;
			}
		} else {
			chain_entry_after_predicate_split = head;
		}
		let _ = chain_entry_after_predicate_split;

		// Lower the match predicate (Check / AnyOf / Not) recursively. Each
		// emitted Check node carries its own on_match / on_miss edges; the
		// combinator tree reshapes those edges per the spec equivalences in
		// C5.5 task 2.
		if let Some(pred) = &rule.raw.match_predicate {
			head = self.lower_predicate(pred, head, on_miss)?;
		}

		Ok(head)
	}

	fn lower_predicate(
		&mut self,
		pred: &Predicate,
		on_match: NodeId,
		on_miss: NodeId,
	) -> Result<NodeId, Error> {
		match pred {
			Predicate::Check(c) => {
				let inst = PredicateInst { path: c.path.clone(), op: compile_operator(&c.op, &c.path)? };
				let pid = self.intern_predicate(inst);
				let collect_body_before =
					if matches!(c.path, FieldPath::HttpBody) { Some(BodySide::Request) } else { None };
				let node = Node::Check { predicate: pid, on_match, on_miss, collect_body_before };
				Ok(self.push_node(node))
			}
			Predicate::AnyOf(any_of) => {
				// any_of [A, B, C] match=>X miss=>Y  ≡
				//     Check(A) match=>X miss=>Check(B) match=>X miss=>Check(C) match=>X miss=>Y
				// Build right-to-left so each preceding Check's on_miss points
				// at the next child's entry.
				if any_of.any_of.is_empty() {
					// Empty any_of is an empty OR — always misses. Spec doesn't
					// call this out; interpret as "never matches" for safety.
					return Ok(on_miss);
				}
				let mut cur_miss = on_miss;
				for child in any_of.any_of.iter().rev() {
					cur_miss = self.lower_predicate(child, on_match, cur_miss)?;
				}
				Ok(cur_miss)
			}
			Predicate::Not(not) => {
				// not P match=>X miss=>Y  ≡  lower(P, match=>Y, miss=>X)
				self.lower_predicate(&not.not, on_miss, on_match)
			}
		}
	}

	fn mark_request_reader(
		&mut self,
		chain_head: NodeId,
		_mw_meta: &dyn MiddlewareMetadataProvider,
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
						if let Node::Middleware { collect_body_before, .. } =
							&mut self.nodes[cur.get() as usize]
						{
							*collect_body_before = Some(BodySide::Request);
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
			| FieldPath::TlsPeerCertSubjectCn
	)
}

fn terminator_for_fetch(kind: FetchKind) -> Terminator {
	match kind {
		FetchKind::HttpProxy | FetchKind::HttpSynthesize => Terminator::WriteHttpResponse,
		FetchKind::L4Forward | FetchKind::WebSocketUpgrade => Terminator::ByteTunnel,
	}
}

type ListenerGroup<'a> = (Vec<SocketAddr>, Vec<&'a AnalyzedRule>);

fn group_by_listener<'a>(rules: &'a [AnalyzedRule]) -> Result<Vec<ListenerGroup<'a>>, Error> {
	let mut groups: HashMap<Vec<SocketAddr>, Vec<&'a AnalyzedRule>> = HashMap::new();
	for rule in rules {
		let mut addrs = Vec::new();
		for spec in &rule.raw.listen {
			for addr in parse_listen(spec)? {
				addrs.push(addr);
			}
		}
		addrs.sort();
		addrs.dedup();
		groups.entry(addrs).or_default().push(rule);
	}
	let mut out: Vec<_> = groups.into_iter().collect();
	out.sort_by(|a, b| a.0.cmp(&b.0));
	Ok(out)
}

fn parse_listen(spec: &str) -> Result<Vec<SocketAddr>, Error> {
	let s = spec.trim();
	// Wildcard-port rejection per 09-config.md.
	if s == ":0" || s == "*:0" {
		return Err(Error::compile(format!("wildcard port rejected: {spec:?}")));
	}
	// Dual-stack shorthand `:443` or `*:443` → v4 + v6.
	if let Some(port_str) = s.strip_prefix(':').or_else(|| s.strip_prefix("*:")) {
		let port =
			u16::from_str(port_str).map_err(|e| Error::compile(format!("bad port in {spec:?}: {e}")))?;
		return Ok(vec![
			SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
			SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port),
		]);
	}
	SocketAddr::from_str(s)
		.map(|a| vec![a])
		.map_err(|e| Error::compile(format!("bad listen spec {spec:?}: {e}")))
}

fn compile_operator(op: &Operator, path: &FieldPath) -> Result<CompiledOperator, Error> {
	Ok(match op {
		Operator::Equals(v) => CompiledOperator::Equals(coerce_value(v, path)?),
		Operator::NotEquals(v) => CompiledOperator::NotEquals(coerce_value(v, path)?),
		Operator::Contains(v) => CompiledOperator::Contains(value_to_bytes(v)?),
		Operator::NotContains(v) => CompiledOperator::NotContains(value_to_bytes(v)?),
		Operator::Prefix(v) => CompiledOperator::Prefix(value_to_bytes(v)?),
		Operator::Suffix(v) => CompiledOperator::Suffix(value_to_bytes(v)?),
		Operator::Matches(pat) => CompiledOperator::Matches(
			fancy_regex::Regex::new(pat).map_err(|e| Error::compile(format!("regex: {e}")))?,
		),
		Operator::In(vs) => {
			let mut out = Vec::with_capacity(vs.len());
			for v in vs {
				out.push(coerce_value(v, path)?);
			}
			CompiledOperator::In(out)
		}
		Operator::NotIn(vs) => {
			let mut out = Vec::with_capacity(vs.len());
			for v in vs {
				out.push(coerce_value(v, path)?);
			}
			CompiledOperator::NotIn(out)
		}
		Operator::Gt(n) => CompiledOperator::Gt(*n),
		Operator::Gte(n) => CompiledOperator::Gte(*n),
		Operator::Lt(n) => CompiledOperator::Lt(*n),
		Operator::Lte(n) => CompiledOperator::Lte(*n),
		Operator::Cidr(s) => CompiledOperator::Cidr(
			ipnet::IpNet::from_str(s).map_err(|e| Error::compile(format!("cidr: {e}")))?,
		),
	})
}

fn coerce_value(v: &Value, path: &FieldPath) -> Result<CompiledValue, Error> {
	// IP-typed paths: parse str → IpAddr.
	let ip_typed = matches!(path, FieldPath::RemoteIp | FieldPath::LocalIp);
	if ip_typed {
		let Value::Str(s) = v else {
			return Err(Error::compile(format!(
				"field {path:?} expects an ip-address string, got {v:?}"
			)));
		};
		return IpAddr::from_str(s)
			.map(CompiledValue::Addr)
			.map_err(|e| Error::compile(format!("bad ip addr: {e}")));
	}
	Ok(match v {
		Value::Str(s) => CompiledValue::Str(Arc::from(s.as_str())),
		Value::Int(n) => CompiledValue::Int(*n),
		Value::Bool(b) => CompiledValue::Bool(*b),
	})
}

fn value_to_bytes(v: &Value) -> Result<bytes::Bytes, Error> {
	match v {
		Value::Str(s) => Ok(bytes::Bytes::copy_from_slice(s.as_bytes())),
		Value::Int(_) | Value::Bool(_) => Err(Error::compile(format!(
			"contains/prefix/suffix expect a string or bytes value, got {v:?}"
		))),
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
