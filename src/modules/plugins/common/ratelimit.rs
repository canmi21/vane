/* src/modules/plugins/common/ratelimit.rs */

use crate::{
	common::getenv,
	modules::plugins::model::{
		GenericMiddleware, Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
	},
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use dashmap::DashMap;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use serde_json::Value;
use std::{any::Any, borrow::Cow, sync::Arc, time::Duration};

// --- Global State ---

/// Storage for per-second rate limiting.
static SEC_POOL: Lazy<Arc<DashMap<String, u32>>> = Lazy::new(|| {
	let map = Arc::new(DashMap::new());
	let map_clone = map.clone();
	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(1));
		loop {
			interval.tick().await;
			if !map_clone.is_empty() {
				map_clone.clear();
			}
		}
	});
	map
});

/// Storage for per-minute rate limiting.
static MIN_POOL: Lazy<Arc<DashMap<String, u32>>> = Lazy::new(|| {
	let map = Arc::new(DashMap::new());
	let map_clone = map.clone();
	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(60));
		loop {
			interval.tick().await;
			if !map_clone.is_empty() {
				map_clone.clear();
			}
		}
	});
	map
});

// --- Helper Functions ---

/// Checks memory usage and prunes entries if the limit is exceeded.
/// Instead of rejecting new keys, it removes a portion of existing keys to make room.
fn ensure_space(map: &DashMap<String, u32>) {
	let max_mem_str = getenv::get_env("MAX_LIMITER_MEMORY", "4194304".to_string()); // Default 4MB
	let max_mem = max_mem_str.parse::<usize>().unwrap_or(4_194_304);

	// Estimate memory usage: len * ~100 bytes overhead per entry
	let estimated_size = map.len() * 100;

	if estimated_size > max_mem {
		log(
			LogLevel::Warn,
			&format!(
				"Rate limiter memory limit exceeded ({} > {} bytes). Pruning 10% of keys to self-preserve.",
				estimated_size, max_mem
			),
		);

		// Prune ~10% of the keys to prevent OOM while keeping the service alive.
		// Since DashMap doesn't track insertion order efficiently, we evict the first
		// batch of keys found in the iterator. In a rate limit scenario, this is an
		// acceptable trade-off for performance.
		let items_to_remove = (map.len() as f64 * 0.1).ceil() as usize;
		let keys_to_remove: Vec<String> = map
			.iter()
			.take(items_to_remove)
			.map(|kv| kv.key().clone())
			.collect();

		for k in keys_to_remove {
			map.remove(&k);
		}
	}
}

fn check_key_length(key: &str) -> bool {
	let max_len_str = getenv::get_env("RATELIMIT_KEY_MAX_LEN", "256".to_string());
	let max_len = max_len_str.parse::<usize>().unwrap_or(256);
	key.len() <= max_len
}

// --- Plugin: Per Second ---

pub struct KeywordRateLimitSecPlugin;

impl Plugin for KeywordRateLimitSecPlugin {
	fn name(&self) -> &'static str {
		"internal.common.ratelimit.sec"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "key".into(),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "limit".into(),
				required: true,
				param_type: ParamType::Integer,
			},
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		Some(self)
	}

	fn as_generic_middleware(&self) -> Option<&dyn GenericMiddleware> {
		Some(self)
	}
}

#[async_trait]
impl GenericMiddleware for KeywordRateLimitSecPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["true".into(), "false".into()]
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		let key = inputs
			.get("key")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Input 'key' missing"))?;

		let limit = inputs
			.get("limit")
			.and_then(Value::as_u64)
			.ok_or_else(|| anyhow!("Input 'limit' missing"))? as u32;

		if !check_key_length(key) {
			return Ok(MiddlewareOutput {
				branch: "false".into(),
				store: None,
			});
		}

		let pool = &*SEC_POOL;

		let current_count = match pool.get_mut(key) {
			Some(mut entry) => {
				*entry += 1;
				*entry
			}
			None => {
				// Ensure space exists (evicting if necessary) before inserting
				ensure_space(pool);
				pool.insert(key.to_string(), 1);
				1
			}
		};

		let branch = if current_count <= limit {
			"true"
		} else {
			"false"
		};

		Ok(MiddlewareOutput {
			branch: branch.into(),
			store: None,
		})
	}
}

#[async_trait]
impl Middleware for KeywordRateLimitSecPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		<Self as GenericMiddleware>::output(self)
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		<Self as GenericMiddleware>::execute(self, inputs).await
	}
}

// --- Plugin: Per Minute ---

pub struct KeywordRateLimitMinPlugin;

impl Plugin for KeywordRateLimitMinPlugin {
	fn name(&self) -> &'static str {
		"internal.common.ratelimit.min"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: "key".into(),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: "limit".into(),
				required: true,
				param_type: ParamType::Integer,
			},
		]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_middleware(&self) -> Option<&dyn Middleware> {
		Some(self)
	}

	fn as_generic_middleware(&self) -> Option<&dyn GenericMiddleware> {
		Some(self)
	}
}

#[async_trait]
impl GenericMiddleware for KeywordRateLimitMinPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec!["true".into(), "false".into()]
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		let key = inputs
			.get("key")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow!("Input 'key' missing"))?;

		let limit = inputs
			.get("limit")
			.and_then(Value::as_u64)
			.ok_or_else(|| anyhow!("Input 'limit' missing"))? as u32;

		if !check_key_length(key) {
			return Ok(MiddlewareOutput {
				branch: "false".into(),
				store: None,
			});
		}

		let pool = &*MIN_POOL;

		let current_count = match pool.get_mut(key) {
			Some(mut entry) => {
				*entry += 1;
				*entry
			}
			None => {
				ensure_space(pool);
				pool.insert(key.to_string(), 1);
				1
			}
		};

		let branch = if current_count <= limit {
			"true"
		} else {
			"false"
		};

		Ok(MiddlewareOutput {
			branch: branch.into(),
			store: None,
		})
	}
}

#[async_trait]
impl Middleware for KeywordRateLimitMinPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		<Self as GenericMiddleware>::output(self)
	}

	async fn execute(&self, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
		<Self as GenericMiddleware>::execute(self, inputs).await
	}
}
