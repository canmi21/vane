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
