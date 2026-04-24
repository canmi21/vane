#[derive(
	Copy, Clone, Eq, PartialEq, Hash, Debug, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct NodeId(u32);

impl NodeId {
	#[must_use]
	pub const fn new(raw: u32) -> Self {
		Self(raw)
	}

	#[must_use]
	pub const fn get(self) -> u32 {
		self.0
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
}
