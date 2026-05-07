//! Live integration tests against the real Cloudflare v4 API.
//!
//! `#[ignore]`-by-default because every run charges a small DNS
//! API quota against the operator's account and requires a real
//! zone they own. CI runs them on-demand via `--include-ignored`
//! when both env vars are set:
//!
//! - `CF_API_TOKEN` — Cloudflare API Token with "Zone DNS Edit"
//!   scope on `CF_TEST_ZONE`.
//! - `CF_TEST_ZONE` — apex of a zone the token can write
//!   (e.g. `vane-test.example`).
//!
//! Each test creates a randomly-named TXT record under
//! `_acme-test-<uuid>.<CF_TEST_ZONE>`, waits for it to propagate
//! through the public resolver pool, and deletes it. The random
//! suffix means concurrent / flaky runs don't step on each other.

#![cfg(feature = "cloudflare")]

use std::time::Duration;

use vane_engine::acme::DnsProvider;
use vane_engine::acme::dns::{CloudflareConfig, CloudflareDnsProvider};

/// Build the provider from env, or `None` when the test should
/// be skipped (env vars unset).
fn provider_or_skip(test_name: &str) -> Option<(CloudflareDnsProvider, String)> {
	let zone = match std::env::var("CF_TEST_ZONE") {
		Ok(z) if !z.is_empty() => z,
		_ => {
			eprintln!("skipping {test_name}: CF_TEST_ZONE not set");
			return None;
		}
	};
	if std::env::var("CF_API_TOKEN").ok().is_none_or(|s| s.is_empty()) {
		eprintln!("skipping {test_name}: CF_API_TOKEN not set");
		return None;
	}
	vane_engine::crypto::install_default_provider();
	let provider = CloudflareDnsProvider::from_config(&CloudflareConfig {
		api_token_env: "CF_API_TOKEN".to_owned(),
		zone_id: None,
	})
	.expect("provider from env");
	Some((provider, zone))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CF_API_TOKEN + CF_TEST_ZONE"]
async fn cloudflare_set_propagate_delete_round_trip() {
	let Some((provider, zone)) = provider_or_skip("cloudflare_set_propagate_delete_round_trip")
	else {
		return;
	};

	// `_acme-test-<random>.<zone>` to avoid collisions with the
	// production `_acme-challenge` namespace. The 12-char hex
	// suffix is long enough to make accidental name collisions
	// across parallel test runs astronomically unlikely.
	let suffix: u64 = rand::random();
	let test_name = format!("_acme-test-{suffix:x}.{zone}");
	let value = format!("vane-cf-test-{suffix:x}");

	provider.set_txt(&test_name, &value).await.expect("set_txt");
	provider
		.wait_propagated(&test_name, &value, Duration::from_mins(2))
		.await
		.expect("wait_propagated within 2 minutes");
	provider.delete_txt(&test_name).await.expect("delete_txt");
}
