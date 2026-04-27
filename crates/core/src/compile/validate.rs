use std::collections::HashSet;

use crate::error::Error;
use crate::ir::{Node, NodeId, SymbolicFlowGraph};
use crate::phase::{Phase, PhaseNodeKind, Transition, transition};

/// Run IR-level structural and phase validation on a freshly-lowered graph.
///
/// # Errors
/// Returns [`Error::compile`] on missing-id references, Fetch edges that
/// don't match the kind's output-mode contract, acyclicity violations, or
/// phase-state-machine mismatches.
pub fn validate(graph: &SymbolicFlowGraph) -> Result<(), Error> {
	check_id_ranges(graph)?;
	check_fetch_edges(graph)?;
	check_acyclic(graph)?;
	check_phases(graph)?;
	Ok(())
}

fn check_id_ranges(graph: &SymbolicFlowGraph) -> Result<(), Error> {
	let n_nodes = u32::try_from(graph.nodes.len()).unwrap_or(u32::MAX);
	let n_preds = u32::try_from(graph.predicates.len()).unwrap_or(u32::MAX);
	let n_mws = u32::try_from(graph.middlewares.len()).unwrap_or(u32::MAX);
	let n_fetches = u32::try_from(graph.fetches.len()).unwrap_or(u32::MAX);
	let n_terms = u32::try_from(graph.terminators.len()).unwrap_or(u32::MAX);

	for (idx, node) in graph.nodes.iter().enumerate() {
		match node {
			Node::Check { predicate, on_match, on_miss, .. } => {
				if predicate.get() >= n_preds {
					return Err(Error::compile(format!(
						"node {idx}: dangling PredicateId({})",
						predicate.get()
					)));
				}
				if on_match.get() >= n_nodes {
					return Err(Error::compile(format!("node {idx}.on_match dangling")));
				}
				if on_miss.get() >= n_nodes {
					return Err(Error::compile(format!("node {idx}.on_miss dangling")));
				}
			}
			Node::Middleware { id, next, on_error, .. } => {
				if id.get() >= n_mws {
					return Err(Error::compile(format!("node {idx}: dangling MiddlewareId({})", id.get())));
				}
				if next.get() >= n_nodes {
					return Err(Error::compile(format!("node {idx}.next dangling")));
				}
				if let Some(e) = on_error
					&& e.get() >= n_nodes
				{
					return Err(Error::compile(format!("node {idx}.on_error dangling")));
				}
			}
			Node::Fetch { id, next_response, next_tunnel, .. } => {
				if id.get() >= n_fetches {
					return Err(Error::compile(format!("node {idx}: dangling FetchId({})", id.get())));
				}
				if let Some(r) = next_response
					&& r.get() >= n_nodes
				{
					return Err(Error::compile(format!("node {idx}.next_response dangling")));
				}
				if let Some(t) = next_tunnel
					&& t.get() >= n_nodes
				{
					return Err(Error::compile(format!("node {idx}.next_tunnel dangling")));
				}
			}
			Node::Upgrade { next } => {
				if next.get() >= n_nodes {
					return Err(Error::compile(format!("node {idx}.next dangling")));
				}
			}
			Node::Terminate(t) => {
				if t.get() >= n_terms {
					return Err(Error::compile(format!("node {idx}: dangling TerminatorId({})", t.get())));
				}
			}
		}
	}
	Ok(())
}

fn check_fetch_edges(graph: &SymbolicFlowGraph) -> Result<(), Error> {
	use crate::fetch::FetchKind::{HttpProxy, HttpSynthesize, L4Forward, WebSocketUpgrade};
	for (idx, node) in graph.nodes.iter().enumerate() {
		let Node::Fetch { id, next_response, next_tunnel, .. } = node else {
			continue;
		};
		let kind = graph[*id].kind;
		match kind {
			HttpProxy | HttpSynthesize => {
				if next_response.is_none() {
					return Err(Error::compile(format!("node {idx}: {kind:?} requires next_response")));
				}
				if next_tunnel.is_some() {
					return Err(Error::compile(format!("node {idx}: {kind:?} must not have next_tunnel")));
				}
			}
			L4Forward => {
				if next_tunnel.is_none() {
					return Err(Error::compile(format!("node {idx}: L4Forward requires next_tunnel")));
				}
				if next_response.is_some() {
					return Err(Error::compile(format!("node {idx}: L4Forward must not have next_response")));
				}
			}
			WebSocketUpgrade => {
				if next_response.is_none() || next_tunnel.is_none() {
					return Err(Error::compile(format!(
						"node {idx}: WebSocketUpgrade requires both next_response and next_tunnel"
					)));
				}
			}
		}
	}
	Ok(())
}

fn check_acyclic(graph: &SymbolicFlowGraph) -> Result<(), Error> {
	#[derive(Copy, Clone)]
	enum Color {
		White,
		Gray,
		Black,
	}
	let mut color: Vec<Color> = (0..graph.nodes.len()).map(|_| Color::White).collect();

	for start in 0..graph.nodes.len() {
		if !matches!(color[start], Color::White) {
			continue;
		}
		let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
		color[start] = Color::Gray;
		while let Some(&(node_idx, child_idx)) = stack.last() {
			let succs = successors(&graph.nodes[node_idx]);
			if child_idx < succs.len() {
				let next = succs[child_idx].get() as usize;
				stack.last_mut().expect("non-empty").1 += 1;
				match color[next] {
					Color::White => {
						color[next] = Color::Gray;
						stack.push((next, 0));
					}
					Color::Gray => {
						return Err(Error::compile(format!("cycle in graph at node {next}")));
					}
					Color::Black => {}
				}
			} else {
				color[node_idx] = Color::Black;
				stack.pop();
			}
		}
	}
	Ok(())
}

fn successors(node: &Node) -> Vec<NodeId> {
	match node {
		Node::Check { on_match, on_miss, .. } => vec![*on_match, *on_miss],
		Node::Middleware { next, on_error, .. } => {
			let mut v = vec![*next];
			if let Some(e) = on_error {
				v.push(*e);
			}
			v
		}
		Node::Fetch { next_response, next_tunnel, .. } => {
			let mut v = Vec::new();
			if let Some(r) = next_response {
				v.push(*r);
			}
			if let Some(t) = next_tunnel {
				v.push(*t);
			}
			v
		}
		Node::Upgrade { next } => vec![*next],
		Node::Terminate(_) => Vec::new(),
	}
}

fn node_kind_for_phase(graph: &SymbolicFlowGraph, node: &Node) -> PhaseNodeKind {
	match node {
		Node::Check { .. } => PhaseNodeKind::Check,
		Node::Middleware { id, .. } => PhaseNodeKind::Middleware(graph[*id].kind),
		Node::Fetch { id, .. } => PhaseNodeKind::Fetch(graph[*id].kind),
		Node::Upgrade { .. } => PhaseNodeKind::Upgrade,
		Node::Terminate(t) => PhaseNodeKind::Terminate(graph[*t]),
	}
}

/// Walk each listener entry through the phase transition table.
///
/// Not invoked from [`validate`] today because MVP graphs lack the
/// `protocol_detect` middleware that advances `L4Raw → L4Peeked` — that
/// middleware lands at S1-16. Callable directly for tests and for future
/// validators that want phase coverage.
///
/// # Errors
/// Returns [`Error::compile`] on phase mismatches per 02-flow.md § _Phase
/// state machine_.
pub fn check_phases(graph: &SymbolicFlowGraph) -> Result<(), Error> {
	let mut seen: HashSet<(NodeId, Phase)> = HashSet::new();
	for &entry in graph.entries.values() {
		visit_phase(graph, entry, Phase::L4Raw, &mut seen)?;
	}
	// Walk every L7 listener's synthesised `Short(Response)` target as a
	// second-class entry rooted at `Phase::L7Response`. The lower pass
	// always emits these as `Terminate(WriteHttpResponse)` nodes —
	// `WriteHttpResponse` accepts `L7Response` per the transition
	// table, so a clean lower produces a clean walk. Bogus entries
	// (a synth target whose terminator is not `WriteHttpResponse`) get
	// caught here with the same "phase mismatch" error shape.
	for &synth in graph.meta.short_circuit_response_entry.values() {
		visit_phase(graph, synth, Phase::L7Response, &mut seen)?;
	}
	Ok(())
}

fn visit_phase(
	graph: &SymbolicFlowGraph,
	id: NodeId,
	phase: Phase,
	seen: &mut HashSet<(NodeId, Phase)>,
) -> Result<(), Error> {
	if !seen.insert((id, phase)) {
		return Ok(());
	}
	let node = &graph[id];
	let kind = node_kind_for_phase(graph, node);
	let t = transition(kind, phase).map_err(|e| {
		Error::compile(format!(
			"phase mismatch at NodeId({}): expected one of {:?}, got {:?}",
			id.get(),
			e.expected,
			e.got,
		))
	})?;
	match (t, node) {
		(Transition::Terminal, _) => Ok(()),
		(Transition::PassThrough, _) => {
			for succ in successors(node) {
				visit_phase(graph, succ, phase, seen)?;
			}
			Ok(())
		}
		(Transition::Into(next_phase), _) => {
			for succ in successors(node) {
				visit_phase(graph, succ, next_phase, seen)?;
			}
			Ok(())
		}
		(
			Transition::BiOutcome { response, tunnel },
			Node::Fetch { next_response, next_tunnel, .. },
		) => {
			if let Some(r) = next_response {
				visit_phase(graph, *r, response, seen)?;
			}
			if let Some(t) = next_tunnel {
				visit_phase(graph, *t, tunnel, seen)?;
			}
			Ok(())
		}
		(Transition::BiOutcome { .. }, _) => {
			Err(Error::compile("BiOutcome transition on non-Fetch node".to_string()))
		}
	}
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;
	use std::path::PathBuf;
	use std::time::SystemTime;

	use super::*;
	use crate::fetch::{FetchKind, SymbolicFetchRef, Terminator};
	use crate::ir::{BodySide, FetchId, FlowGraphMeta, PredicateId, TerminatorId};

	fn empty_meta() -> FlowGraphMeta {
		FlowGraphMeta {
			version_hash: [0; 32],
			compiled_at: SystemTime::UNIX_EPOCH,
			source_files: vec![PathBuf::new()],
			feature_set: &[],
			short_circuit_response_entry: std::collections::BTreeMap::new(),
			listener_tls: std::collections::BTreeMap::new(),
		}
	}

	#[test]
	fn dangling_terminator_id_in_terminate_node_rejected() {
		let graph = SymbolicFlowGraph {
			nodes: vec![Node::Terminate(TerminatorId::new(0))],
			predicates: vec![],
			middlewares: vec![],
			fetches: vec![],
			terminators: vec![],
			entries: HashMap::new(),
			meta: empty_meta(),
		};
		let err = validate(&graph).expect_err("must error");
		assert!(err.to_string().contains("dangling TerminatorId"));
	}

	#[test]
	fn dangling_node_id_in_fetch_edge_rejected() {
		let graph = SymbolicFlowGraph {
			nodes: vec![Node::Fetch {
				id: FetchId::new(0),
				next_response: Some(NodeId::new(99)),
				next_tunnel: None,
				collect_body_before: None,
				body_limit: 0,
			}],
			predicates: vec![],
			middlewares: vec![],
			fetches: vec![SymbolicFetchRef { kind: FetchKind::HttpProxy, args: serde_json::Value::Null }],
			terminators: vec![],
			entries: HashMap::new(),
			meta: empty_meta(),
		};
		let err = validate(&graph).expect_err("must error");
		assert!(err.to_string().contains("next_response dangling"));
	}

	#[test]
	fn http_fetch_without_next_response_rejected() {
		let term = Node::Terminate(TerminatorId::new(0));
		let graph = SymbolicFlowGraph {
			nodes: vec![
				term,
				Node::Fetch {
					id: FetchId::new(0),
					next_response: None,
					next_tunnel: None,
					collect_body_before: None,
					body_limit: 0,
				},
			],
			predicates: vec![],
			middlewares: vec![],
			fetches: vec![SymbolicFetchRef { kind: FetchKind::HttpProxy, args: serde_json::Value::Null }],
			terminators: vec![Terminator::WriteHttpResponse],
			entries: HashMap::new(),
			meta: empty_meta(),
		};
		let err = validate(&graph).expect_err("must error");
		assert!(err.to_string().contains("requires next_response"));
	}

	#[test]
	fn l4_forward_with_next_response_rejected() {
		let graph = SymbolicFlowGraph {
			nodes: vec![
				Node::Terminate(TerminatorId::new(0)),
				Node::Fetch {
					id: FetchId::new(0),
					next_response: Some(NodeId::new(0)),
					next_tunnel: Some(NodeId::new(0)),
					collect_body_before: None,
					body_limit: 0,
				},
			],
			predicates: vec![],
			middlewares: vec![],
			fetches: vec![SymbolicFetchRef { kind: FetchKind::L4Forward, args: serde_json::Value::Null }],
			terminators: vec![Terminator::ByteTunnel],
			entries: HashMap::new(),
			meta: empty_meta(),
		};
		let err = validate(&graph).expect_err("must error");
		assert!(err.to_string().contains("L4Forward must not have next_response"));
	}

	#[test]
	fn cyclic_graph_is_rejected() {
		// Node 0 and Node 1 point at each other via Check on_match edges.
		let graph = SymbolicFlowGraph {
			nodes: vec![
				Node::Check {
					predicate: PredicateId::new(0),
					on_match: NodeId::new(1),
					on_miss: NodeId::new(1),
					collect_body_before: None,
					body_limit: 0,
				},
				Node::Check {
					predicate: PredicateId::new(0),
					on_match: NodeId::new(0),
					on_miss: NodeId::new(0),
					collect_body_before: None,
					body_limit: 0,
				},
			],
			predicates: vec![dummy_predicate()],
			middlewares: vec![],
			fetches: vec![],
			terminators: vec![],
			entries: HashMap::new(),
			meta: empty_meta(),
		};
		let err = validate(&graph).expect_err("must error");
		assert!(err.to_string().contains("cycle"));
	}

	#[test]
	fn phase_check_rejects_write_http_response_reached_in_wrong_phase() {
		// Upgrade out-phase is L7Request (spec C5.5 patch accepts L4Raw in);
		// Terminate(WriteHttpResponse) requires L7Response — so walking
		// Upgrade directly into it is a phase mismatch the validator must
		// catch.
		let tid = TerminatorId::new(0);
		let graph = SymbolicFlowGraph {
			nodes: vec![Node::Terminate(tid), Node::Upgrade { next: NodeId::new(0) }],
			predicates: vec![],
			middlewares: vec![],
			fetches: vec![],
			terminators: vec![Terminator::WriteHttpResponse],
			entries: {
				let mut m = HashMap::new();
				m.insert("127.0.0.1:443".parse().expect("parse"), NodeId::new(1));
				m
			},
			meta: empty_meta(),
		};
		let err = check_phases(&graph).expect_err("must error");
		assert!(err.to_string().contains("phase mismatch"));
	}

	#[test]
	fn phase_check_rejects_short_circuit_synth_with_wrong_terminator() {
		// `meta.short_circuit_response_entry` values are walked at
		// `Phase::L7Response`. A synth target whose terminator does not
		// accept that phase must trip the same "phase mismatch" error
		// the standard walker uses. `Terminator::Close` is phase-agnostic
		// so it would never trip this check; `ByteTunnel` only accepts
		// `Phase::Tunnel` and is the right negative-test fixture.
		let bad_tid = TerminatorId::new(0);
		let mut meta = empty_meta();
		meta.short_circuit_response_entry.insert(NodeId::new(1), NodeId::new(0));
		let graph = SymbolicFlowGraph {
			nodes: vec![Node::Terminate(bad_tid), Node::Upgrade { next: NodeId::new(0) }],
			predicates: vec![],
			middlewares: vec![],
			fetches: vec![],
			terminators: vec![Terminator::ByteTunnel],
			// No `entries` — exercise the synth walk in isolation.
			entries: HashMap::new(),
			meta,
		};
		let err = check_phases(&graph).expect_err("must error on bad synth phase");
		assert!(err.to_string().contains("phase mismatch"), "{err}");
	}

	fn dummy_predicate() -> crate::predicate::PredicateInst {
		use crate::predicate::{CompiledOperator, CompiledValue, FieldPath, PredicateInst};
		PredicateInst {
			path: FieldPath::TlsSni,
			op: CompiledOperator::Equals(CompiledValue::Str(std::sync::Arc::from("x"))),
		}
	}

	// `BodySide` import is kept here to keep test doc consistent with the
	// `Node` field it accesses in the broader impl.
	const _: BodySide = BodySide::Request;
}
