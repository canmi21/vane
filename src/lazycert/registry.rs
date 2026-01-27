/* src/lazycert/registry.rs */

use dashmap::DashMap;
use once_cell::sync::Lazy;

/// Global registry for active HTTP-01 challenges
/// Maps token -> key_authorization
pub static CHALLENGE_REGISTRY: Lazy<DashMap<String, ChallengeEntry>> = Lazy::new(DashMap::new);

#[derive(Clone, Debug)]
pub struct ChallengeEntry {
	pub key_authorization: String,
	pub domain: String,
	pub challenge_id: String,
}

impl ChallengeEntry {
	#[must_use]
	pub fn new(key_authorization: String, domain: String, challenge_id: String) -> Self {
		Self {
			key_authorization,
			domain,
			challenge_id,
		}
	}
}
