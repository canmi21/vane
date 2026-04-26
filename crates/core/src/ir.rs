use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::Index;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::fetch::{SymbolicFetchRef, Terminator};
use crate::middleware::SymbolicMiddlewareRef;
use crate::predicate::PredicateInst;

macro_rules! id_newtype {
	($name:ident) => {
		#[derive(
			Copy, Clone, Eq, PartialEq, Hash, Debug, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
		)]
		pub struct $name(u32);

		impl $name {
			#[must_use]
			pub const fn new(raw: u32) -> Self {
				Self(raw)
			}

			#[must_use]
			pub const fn get(self) -> u32 {
				self.0
			}
		}
	};
}

id_newtype!(NodeId);
id_newtype!(PredicateId);
id_newtype!(MiddlewareId);
id_newtype!(FetchId);
id_newtype!(TerminatorId);

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum BodySide {
	Request,
	Response,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Node {
	Check {
		predicate: PredicateId,
		on_match: NodeId,
		on_miss: NodeId,
		collect_body_before: Option<BodySide>,
	},
	Middleware {
		id: MiddlewareId,
		next: NodeId,
		on_error: Option<NodeId>,
		collect_body_before: Option<BodySide>,
	},
	Fetch {
		id: FetchId,
		next_response: Option<NodeId>,
		next_tunnel: Option<NodeId>,
		collect_body_before: Option<BodySide>,
	},
	Upgrade {
		next: NodeId,
	},
	Terminate(TerminatorId),
}

impl Node {
	#[must_use]
	pub const fn collect_body_before(&self) -> Option<BodySide> {
		match self {
			Self::Check { collect_body_before, .. }
			| Self::Middleware { collect_body_before, .. }
			| Self::Fetch { collect_body_before, .. } => *collect_body_before,
			Self::Upgrade { .. } | Self::Terminate(_) => None,
		}
	}
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FlowGraphMeta {
	pub version_hash: [u8; 32],
	pub compiled_at: SystemTime,
	pub source_files: Vec<PathBuf>,
	// `feature_set` is a compile-time slice the daemon fills in at link, not
	// a user-authored value; dry-run JSON omits it and deserialization
	// restores the empty slice. Engine's link step installs the real value.
	#[serde(skip, default = "empty_feature_set")]
	pub feature_set: &'static [&'static str],

	/// Map of L7-listener entry `NodeId` → synthesised
	/// `Terminate(WriteHttpResponse)` `NodeId`. The executor jumps here
	/// when an L7 request middleware returns
	/// `Decision::Short(ShortCircuit::Response(_))`: it sets the response
	/// slot and walks to the synth target so the response runs through
	/// the standard `WriteHttpResponse` write path. Empty for L4-only
	/// graphs and for any L7 entry whose listener is not bound to a
	/// post-`Upgrade` chain (which the lower pass guarantees never
	/// happens for legal L7 listeners). See spec/architecture/02-flow.md
	/// § _`FlowGraph` metadata_.
	///
	/// `#[serde(default)]` keeps older dry-run JSON snapshots
	/// deserializable: missing field decodes as an empty map, which
	/// matches the legacy "no L7 listeners" graph shape.
	#[serde(default)]
	pub short_circuit_response_entry: std::collections::BTreeMap<NodeId, NodeId>,
}

const fn empty_feature_set() -> &'static [&'static str] {
	&[]
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SymbolicFlowGraph {
	pub nodes: Vec<Node>,
	pub predicates: Vec<PredicateInst>,
	pub middlewares: Vec<SymbolicMiddlewareRef>,
	pub fetches: Vec<SymbolicFetchRef>,
	pub terminators: Vec<Terminator>,
	pub entries: HashMap<SocketAddr, NodeId>,
	pub meta: FlowGraphMeta,
}

impl Index<NodeId> for SymbolicFlowGraph {
	type Output = Node;
	fn index(&self, id: NodeId) -> &Node {
		&self.nodes[id.get() as usize]
	}
}

impl Index<PredicateId> for SymbolicFlowGraph {
	type Output = PredicateInst;
	fn index(&self, id: PredicateId) -> &PredicateInst {
		&self.predicates[id.get() as usize]
	}
}

impl Index<MiddlewareId> for SymbolicFlowGraph {
	type Output = SymbolicMiddlewareRef;
	fn index(&self, id: MiddlewareId) -> &SymbolicMiddlewareRef {
		&self.middlewares[id.get() as usize]
	}
}

impl Index<FetchId> for SymbolicFlowGraph {
	type Output = SymbolicFetchRef;
	fn index(&self, id: FetchId) -> &SymbolicFetchRef {
		&self.fetches[id.get() as usize]
	}
}

impl Index<TerminatorId> for SymbolicFlowGraph {
	type Output = Terminator;
	fn index(&self, id: TerminatorId) -> &Terminator {
		&self.terminators[id.get() as usize]
	}
}

#[cfg(test)]
mod tests {
	use std::collections::hash_map::DefaultHasher;
	use std::hash::{Hash, Hasher};
	use std::sync::Arc;

	use serde_json::Value;

	use super::*;
	use crate::fetch::{FetchKind, SymbolicFetchRef, Terminator};
	use crate::middleware::{MiddlewareKind, SymbolicMiddlewareRef};
	use crate::predicate::{CompiledOperator, CompiledValue, FieldPath, PredicateInst};

	#[test]
	fn new_then_get_round_trips_raw_u32() {
		for raw in [0_u32, 1, 42, u32::MAX] {
			assert_eq!(NodeId::new(raw).get(), raw);
		}
	}

	#[test]
	fn node_id_equality_is_structural() {
		assert_eq!(NodeId::new(7), NodeId::new(7));
		assert_ne!(NodeId::new(7), NodeId::new(8));
	}

	#[test]
	fn node_id_ordering_follows_raw_u32() {
		assert!(NodeId::new(1) < NodeId::new(2));
		assert!(NodeId::new(u32::MAX) > NodeId::new(0));
	}

	#[test]
	fn node_id_serde_round_trip() {
		let id = NodeId::new(0x0bad_f00d);
		let encoded = serde_json::to_string(&id).expect("serialize");
		let decoded: NodeId = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, id);
	}

	#[test]
	fn body_side_serde_round_trip_per_variant() {
		for s in [BodySide::Request, BodySide::Response] {
			let encoded = serde_json::to_string(&s).expect("serialize");
			let decoded: BodySide = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, s);
		}
	}

	fn hash_of<T: Hash>(t: &T) -> u64 {
		let mut h = DefaultHasher::new();
		t.hash(&mut h);
		h.finish()
	}

	#[test]
	fn predicate_id_new_get_round_trip_and_hash_eq() {
		for raw in [0_u32, 1, 42, u32::MAX] {
			let a = PredicateId::new(raw);
			let b = PredicateId::new(raw);
			assert_eq!(a.get(), raw);
			assert_eq!(a, b);
			assert_eq!(hash_of(&a), hash_of(&b));
			let encoded = serde_json::to_string(&a).expect("serialize");
			let decoded: PredicateId = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, a);
		}
	}

	#[test]
	fn middleware_id_new_get_round_trip_and_hash_eq() {
		for raw in [0_u32, 1, 42, u32::MAX] {
			let a = MiddlewareId::new(raw);
			let b = MiddlewareId::new(raw);
			assert_eq!(a.get(), raw);
			assert_eq!(a, b);
			assert_eq!(hash_of(&a), hash_of(&b));
			let encoded = serde_json::to_string(&a).expect("serialize");
			let decoded: MiddlewareId = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, a);
		}
	}

	#[test]
	fn fetch_id_new_get_round_trip_and_hash_eq() {
		for raw in [0_u32, 1, 42, u32::MAX] {
			let a = FetchId::new(raw);
			let b = FetchId::new(raw);
			assert_eq!(a.get(), raw);
			assert_eq!(a, b);
			assert_eq!(hash_of(&a), hash_of(&b));
			let encoded = serde_json::to_string(&a).expect("serialize");
			let decoded: FetchId = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, a);
		}
	}

	#[test]
	fn terminator_id_new_get_round_trip_and_hash_eq() {
		for raw in [0_u32, 1, 42, u32::MAX] {
			let a = TerminatorId::new(raw);
			let b = TerminatorId::new(raw);
			assert_eq!(a.get(), raw);
			assert_eq!(a, b);
			assert_eq!(hash_of(&a), hash_of(&b));
			let encoded = serde_json::to_string(&a).expect("serialize");
			let decoded: TerminatorId = serde_json::from_str(&encoded).expect("deserialize");
			assert_eq!(decoded, a);
		}
	}

	// The newtype wrappers are distinct types — a function accepting `NodeId`
	// refuses a `PredicateId` at compile time. `_id_types_are_distinct` is a
	// compile-only witness that the signatures pin the right types; any mix-up
	// at a call site would fail to type-check.
	fn _id_types_are_distinct(
		_n: NodeId,
		_p: PredicateId,
		_m: MiddlewareId,
		_f: FetchId,
		_t: TerminatorId,
	) {
	}

	#[test]
	fn node_check_collect_body_before_returns_stored_flag() {
		let some = Node::Check {
			predicate: PredicateId::new(0),
			on_match: NodeId::new(0),
			on_miss: NodeId::new(0),
			collect_body_before: Some(BodySide::Request),
		};
		assert_eq!(some.collect_body_before(), Some(BodySide::Request));

		let none = Node::Check {
			predicate: PredicateId::new(0),
			on_match: NodeId::new(0),
			on_miss: NodeId::new(0),
			collect_body_before: None,
		};
		assert_eq!(none.collect_body_before(), None);
	}

	#[test]
	fn node_middleware_collect_body_before_returns_stored_flag() {
		let some = Node::Middleware {
			id: MiddlewareId::new(0),
			next: NodeId::new(0),
			on_error: None,
			collect_body_before: Some(BodySide::Response),
		};
		assert_eq!(some.collect_body_before(), Some(BodySide::Response));

		let none = Node::Middleware {
			id: MiddlewareId::new(0),
			next: NodeId::new(0),
			on_error: None,
			collect_body_before: None,
		};
		assert_eq!(none.collect_body_before(), None);
	}

	#[test]
	fn node_fetch_collect_body_before_returns_stored_flag() {
		let some = Node::Fetch {
			id: FetchId::new(0),
			next_response: None,
			next_tunnel: None,
			collect_body_before: Some(BodySide::Request),
		};
		assert_eq!(some.collect_body_before(), Some(BodySide::Request));

		let none = Node::Fetch {
			id: FetchId::new(0),
			next_response: None,
			next_tunnel: None,
			collect_body_before: None,
		};
		assert_eq!(none.collect_body_before(), None);
	}

	#[test]
	fn node_upgrade_collect_body_before_is_always_none() {
		let n = Node::Upgrade { next: NodeId::new(0) };
		assert_eq!(n.collect_body_before(), None);
	}

	#[test]
	fn node_terminate_collect_body_before_is_always_none() {
		let n = Node::Terminate(TerminatorId::new(0));
		assert_eq!(n.collect_body_before(), None);
	}

	fn sample_predicate() -> PredicateInst {
		PredicateInst {
			path: FieldPath::TlsSni,
			op: CompiledOperator::Equals(CompiledValue::Str(Arc::from("a"))),
		}
	}

	fn sample_middleware() -> SymbolicMiddlewareRef {
		SymbolicMiddlewareRef {
			name: Arc::from("noop"),
			args: Value::Null,
			kind: MiddlewareKind::L7Request,
			stateless: true,
			needs_body: false,
			on_error: None,
		}
	}

	fn sample_fetch() -> SymbolicFetchRef {
		SymbolicFetchRef { kind: FetchKind::HttpProxy, args: Value::Null }
	}

	fn sample_meta() -> FlowGraphMeta {
		FlowGraphMeta {
			version_hash: [0; 32],
			compiled_at: SystemTime::UNIX_EPOCH,
			source_files: vec![],
			feature_set: &[],
			short_circuit_response_entry: std::collections::BTreeMap::new(),
		}
	}

	fn one_of_each_graph() -> SymbolicFlowGraph {
		SymbolicFlowGraph {
			nodes: vec![Node::Terminate(TerminatorId::new(0))],
			predicates: vec![sample_predicate()],
			middlewares: vec![sample_middleware()],
			fetches: vec![sample_fetch()],
			terminators: vec![Terminator::WriteHttpResponse],
			entries: HashMap::new(),
			meta: sample_meta(),
		}
	}

	#[test]
	fn index_by_node_id_returns_matching_node() {
		let g = one_of_each_graph();
		match &g[NodeId::new(0)] {
			Node::Terminate(t) => assert_eq!(*t, TerminatorId::new(0)),
			other => panic!("expected Terminate, got {other:?}"),
		}
	}

	#[test]
	fn index_by_predicate_id_returns_matching_predicate() {
		let g = one_of_each_graph();
		assert_eq!(g[PredicateId::new(0)], sample_predicate());
	}

	#[test]
	fn index_by_middleware_id_returns_matching_middleware() {
		let g = one_of_each_graph();
		assert_eq!(g[MiddlewareId::new(0)], sample_middleware());
	}

	#[test]
	fn index_by_fetch_id_returns_matching_fetch() {
		let g = one_of_each_graph();
		assert_eq!(g[FetchId::new(0)].kind, FetchKind::HttpProxy);
	}

	#[test]
	fn index_by_terminator_id_returns_matching_terminator() {
		let g = one_of_each_graph();
		assert_eq!(g[TerminatorId::new(0)], Terminator::WriteHttpResponse);
	}

	fn node_round_trip(n: &Node) -> Node {
		let encoded = serde_json::to_string(n).expect("serialize node");
		serde_json::from_str(&encoded).expect("deserialize node")
	}

	#[test]
	fn node_check_serde_round_trip_with_and_without_collect_flag() {
		let with = Node::Check {
			predicate: PredicateId::new(3),
			on_match: NodeId::new(4),
			on_miss: NodeId::new(5),
			collect_body_before: Some(BodySide::Request),
		};
		match node_round_trip(&with) {
			Node::Check { predicate, on_match, on_miss, collect_body_before } => {
				assert_eq!(predicate, PredicateId::new(3));
				assert_eq!(on_match, NodeId::new(4));
				assert_eq!(on_miss, NodeId::new(5));
				assert_eq!(collect_body_before, Some(BodySide::Request));
			}
			other => panic!("expected Check, got {other:?}"),
		}

		let without = Node::Check {
			predicate: PredicateId::new(0),
			on_match: NodeId::new(0),
			on_miss: NodeId::new(0),
			collect_body_before: None,
		};
		match node_round_trip(&without) {
			Node::Check { collect_body_before, .. } => assert_eq!(collect_body_before, None),
			other => panic!("expected Check, got {other:?}"),
		}
	}

	#[test]
	fn node_middleware_serde_round_trip_with_and_without_collect_flag() {
		let with = Node::Middleware {
			id: MiddlewareId::new(1),
			next: NodeId::new(2),
			on_error: Some(NodeId::new(9)),
			collect_body_before: Some(BodySide::Response),
		};
		match node_round_trip(&with) {
			Node::Middleware { id, next, on_error, collect_body_before } => {
				assert_eq!(id, MiddlewareId::new(1));
				assert_eq!(next, NodeId::new(2));
				assert_eq!(on_error, Some(NodeId::new(9)));
				assert_eq!(collect_body_before, Some(BodySide::Response));
			}
			other => panic!("expected Middleware, got {other:?}"),
		}

		let without = Node::Middleware {
			id: MiddlewareId::new(0),
			next: NodeId::new(0),
			on_error: None,
			collect_body_before: None,
		};
		match node_round_trip(&without) {
			Node::Middleware { on_error, collect_body_before, .. } => {
				assert_eq!(on_error, None);
				assert_eq!(collect_body_before, None);
			}
			other => panic!("expected Middleware, got {other:?}"),
		}
	}

	#[test]
	fn node_fetch_serde_round_trip_with_and_without_collect_flag() {
		let with = Node::Fetch {
			id: FetchId::new(7),
			next_response: Some(NodeId::new(8)),
			next_tunnel: Some(NodeId::new(9)),
			collect_body_before: Some(BodySide::Request),
		};
		match node_round_trip(&with) {
			Node::Fetch { id, next_response, next_tunnel, collect_body_before } => {
				assert_eq!(id, FetchId::new(7));
				assert_eq!(next_response, Some(NodeId::new(8)));
				assert_eq!(next_tunnel, Some(NodeId::new(9)));
				assert_eq!(collect_body_before, Some(BodySide::Request));
			}
			other => panic!("expected Fetch, got {other:?}"),
		}

		let without = Node::Fetch {
			id: FetchId::new(0),
			next_response: None,
			next_tunnel: None,
			collect_body_before: None,
		};
		match node_round_trip(&without) {
			Node::Fetch { next_response, next_tunnel, collect_body_before, .. } => {
				assert_eq!(next_response, None);
				assert_eq!(next_tunnel, None);
				assert_eq!(collect_body_before, None);
			}
			other => panic!("expected Fetch, got {other:?}"),
		}
	}

	#[test]
	fn node_upgrade_serde_round_trip() {
		let n = Node::Upgrade { next: NodeId::new(11) };
		match node_round_trip(&n) {
			Node::Upgrade { next } => assert_eq!(next, NodeId::new(11)),
			other => panic!("expected Upgrade, got {other:?}"),
		}
	}

	#[test]
	fn node_terminate_serde_round_trip() {
		let n = Node::Terminate(TerminatorId::new(13));
		match node_round_trip(&n) {
			Node::Terminate(t) => assert_eq!(t, TerminatorId::new(13)),
			other => panic!("expected Terminate, got {other:?}"),
		}
	}

	// `FlowGraphMeta` derives `Serialize` but not `Deserialize` (the spec
	// comment in this module notes `Deserialize` lands with S1-32). Assert the
	// forward direction only.
	#[test]
	fn flow_graph_meta_serializes_and_emits_version_hash_field() {
		let meta = sample_meta();
		let encoded = serde_json::to_string(&meta).expect("serialize meta");
		assert!(encoded.contains("version_hash"), "expected version_hash field in {encoded}");
	}

	#[test]
	fn flow_graph_meta_round_trip_preserves_all_but_feature_set() {
		// 02-flow.md § _FlowGraph metadata_: feature_set is a compile-time
		// slice the daemon fills in at link and is NOT emitted to dry-run JSON.
		// version_hash / compiled_at / source_files must round-trip.
		use std::time::Duration;
		let meta = FlowGraphMeta {
			version_hash: [0x42; 32],
			compiled_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000),
			source_files: vec![PathBuf::from("/a.json"), PathBuf::from("/b.json")],
			feature_set: &["h3", "wasm"],
			short_circuit_response_entry: std::collections::BTreeMap::new(),
		};
		let encoded = serde_json::to_string(&meta).expect("serialize meta");
		assert!(
			!encoded.contains("feature_set"),
			"feature_set must be skipped in dry-run JSON, got: {encoded}",
		);
		let decoded: FlowGraphMeta = serde_json::from_str(&encoded).expect("deserialize meta");
		assert_eq!(decoded.version_hash, meta.version_hash);
		assert_eq!(decoded.compiled_at, meta.compiled_at);
		assert_eq!(decoded.source_files, meta.source_files);
		// feature_set is restored to the empty slice by #[serde(skip, default=...)].
		assert!(decoded.feature_set.is_empty(), "feature_set must default to empty on deserialize");
	}
}
