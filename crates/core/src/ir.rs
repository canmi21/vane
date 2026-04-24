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

#[derive(Clone, Debug, serde::Serialize)]
pub struct FlowGraphMeta {
	pub version_hash: [u8; 32],
	pub compiled_at: SystemTime,
	pub source_files: Vec<PathBuf>,
	pub feature_set: &'static [&'static str],
}

// Full `serde::Serialize` on `SymbolicFlowGraph` lands with `vane compile
// --dry-run` (S1-32). It requires wiring Serialize through `PredicateInst` /
// `CompiledOperator::Matches(fancy_regex::Regex)` / `CompiledValue::Bytes`,
// which are non-trivial. This chunk builds the struct; serialization comes
// when the CLI needs it.
#[derive(Clone, Debug)]
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
	use super::*;

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
}
