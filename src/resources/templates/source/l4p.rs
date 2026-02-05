/* src/resources/templates/source/l4p.rs */

use crate::resources::kv::KvStore;
use ahash::AHashMap;
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::RwLock;
use varchain::{Resolved, Source, SourceFuture};

pub struct L4PlusSource {
	pub kv: Arc<RwLock<KvStore>>,
	pub payloads: Arc<AHashMap<String, Bytes>>,
}

impl Source for L4PlusSource {
	fn get(&self, key: &str) -> SourceFuture<'_, String> {
		let key = key.to_owned();
		let kv = self.kv.clone();
		let payloads = self.payloads.clone();

		Box::pin(async move {
			if !matches!(key.as_str(), "tls.clienthello" | "quic.initial") {
				return Resolved::Pass;
			}

			// 1. Check if already in KV (cached hex)
			{
				let kv_read = kv.read().await;
				if let Some(cached) = kv_read.get(&key) {
					return Resolved::Found(cached.clone());
				}
			}

			// 2. Resolve from raw payloads
			if let Some(raw) = payloads.get(&key) {
				let hex_encoded = hex::encode(raw);

				// 3. Cache back to KV
				let mut kv_write = kv.write().await;
				kv_write.insert(key, hex_encoded.clone());

				return Resolved::Found(hex_encoded);
			}

			Resolved::Pass
		})
	}
}
