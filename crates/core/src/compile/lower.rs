use std::collections::{BTreeMap, HashMap};
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
	for (addrs, rules) in groups {
		// TLS termination is per-listener, not per-rule: every rule
		// sharing an address contributes to the listener's cert pool.
		// `resolve_listener_tls` aggregates and rejects conflicts —
		// see 08-tls.md § _TLS termination_ + § _Certificate resolver_.
		let resolved_tls = resolve_listener_tls(&addrs, &rules)?;
		let entry = builder.lower_port(&rules, mw_meta, fetch_meta)?;
		for addr in &addrs {
			builder.entries.insert(*addr, entry);
		}
		if let Some(spec) = resolved_tls {
			for addr in addrs {
				builder.listener_tls.insert(addr, spec.clone());
			}
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
			short_circuit_response_entry: builder.short_circuit_response_entry,
			listener_tls: builder.listener_tls,
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
		let fid =
			self.push_fetch(SymbolicFetchRef { kind: fetch_kind, args: rule.raw.terminate.args.clone() });
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
				let node = Node::Check { predicate: pid, on_match, on_miss, collect_body_before };
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
		| FieldPath::TlsPeerCertSubjectCn => Level::L4Peek,
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
			| FieldPath::TlsPeerCertSubjectCn
	)
}

type ListenerGroup<'a> = (Vec<SocketAddr>, Vec<&'a AnalyzedRule>);

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

	let mut spec = crate::rule::ListenerTlsSpec { default: None, sni_certs: BTreeMap::new() };
	for rule in rules {
		let Some(tls) = rule.raw.tls.as_ref() else { continue };
		match tls.sni.as_deref() {
			None => {
				let normalised = crate::rule::TlsConfig {
					sni: None,
					cert_file: tls.cert_file.clone(),
					key_file: tls.key_file.clone(),
				};
				match &spec.default {
					None => spec.default = Some(normalised),
					Some(existing) if existing == &normalised => {}
					Some(existing) => {
						return Err(Error::compile(format!(
							"listener {addrs:?}: more than one default (sni-less) cert — {} vs {} — at most one cert may omit `sni`",
							existing.cert_file.display(),
							normalised.cert_file.display(),
						)));
					}
				}
			}
			Some(sni_raw) => {
				let sni_key = sni_raw.to_ascii_lowercase();
				let normalised = crate::rule::TlsConfig {
					sni: Some(sni_key.clone()),
					cert_file: tls.cert_file.clone(),
					key_file: tls.key_file.clone(),
				};
				match spec.sni_certs.get(&sni_key) {
					None => {
						spec.sni_certs.insert(sni_key, normalised);
					}
					Some(existing) if existing == &normalised => {}
					Some(existing) => {
						return Err(Error::compile(format!(
							"listener {addrs:?}: SNI {sni_key:?} mapped to two different certs — {} vs {}",
							existing.cert_file.display(),
							normalised.cert_file.display(),
						)));
					}
				}
			}
		}
	}

	if spec.is_empty() { Ok(None) } else { Ok(Some(spec)) }
}

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
			// Spec encodes bytes-typed values as base64 strings. Existing
			// preset compile / round-trip tests rely on the simpler "raw
			// bytes from the JSON string literal" coercion; that pre-
			// existing shortcut is preserved here. Round-tripping the IR
			// through the shadow-enum (de_bytes) does decode base64 on
			// the IR side — only the lower-time path is the shortcut.
			let Value::Str(s) = v else {
				return Err(mismatch());
			};
			Ok(CompiledValue::Bytes(bytes::Bytes::copy_from_slice(s.as_bytes())))
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
	match v {
		Value::Str(s) => Ok(bytes::Bytes::copy_from_slice(s.as_bytes())),
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
		assert!(msg.contains("Str or Bytes"), "{msg}");
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
		assert!(err.to_string().contains("Str or Bytes"));
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
}
