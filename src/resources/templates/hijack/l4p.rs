/* src/resources/templates/hijack/l4p.rs */

use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;

use super::Hijacker;
use crate::resources::kv::KvStore;

/// L4+ carrier hijacker for lazy hex encoding
pub struct L4PlusHijacker<'a> {
	pub kv: &'a mut KvStore,
	pub payloads: &'a ahash::AHashMap<String, Bytes>,
}

#[async_trait]
impl<'a> Hijacker for L4PlusHijacker<'a> {
	fn can_handle(&self, key: &str) -> bool {
		matches!(key, "tls.clienthello" | "quic.initial")
	}

	async fn resolve(&mut self, key: &str) -> Result<String> {
		// 1. Check if already in KV (cached hex)
		if let Some(cached) = self.kv.get(key) {
			return Ok(cached.clone());
		}

		// 2. Resolve from raw payloads
		if let Some(raw) = self.payloads.get(key) {
			let hex_encoded = hex::encode(raw);

			// 3. Cache back to KV for future lookups in the same flow
			self.kv.insert(key.to_owned(), hex_encoded.clone());

			return Ok(hex_encoded);
		}

		anyhow::bail!("Raw data for key '{key}' not found in L4+ context")
	}
}
