/* src/primitives/src/lazycert.rs */

use dashmap::DashMap;
use std::sync::LazyLock;

/// Global registry for active HTTP-01 challenges
/// Maps token -> key_authorization
pub static CHALLENGE_REGISTRY: LazyLock<DashMap<String, ChallengeEntry>> =
	LazyLock::new(DashMap::new);

#[derive(Clone, Debug)]
pub struct ChallengeEntry {
	pub key_authorization: String,
	pub domain: String,
	pub challenge_id: String,
}

impl ChallengeEntry {
	#[must_use]
	pub fn new(key_authorization: String, domain: String, challenge_id: String) -> Self {
		Self { key_authorization, domain, challenge_id }
	}
}
