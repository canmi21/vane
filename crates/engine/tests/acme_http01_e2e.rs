//! End-to-end tests for HTTP-01 ACME issuance against
//! [Pebble](https://github.com/letsencrypt/pebble), Let's
//! Encrypt's official test ACME server.
//!
//! Covers the `instant-acme` round-trip — account create, order,
//! finalize, cert download — driven through
//! [`vane_engine::acme::ManagedCertRegistry::issue_http01_with_root`].
//! The Pebble fixture runs in `PEBBLE_VA_ALWAYS_VALID=1` mode so the
//! actual challenge fetch isn't exercised here; the challenge-handler
//! unit tests in `crates/engine/src/fetch/acme_challenge.rs` cover
//! that independently.
//!
//! Both tests are `#[ignore]`'d so the default
//! `cargo nextest run --workspace` doesn't fail on machines
//! without a Docker daemon. Run them explicitly with:
//!
//! ```text
//! cargo nextest run --features acme --test acme_http01_e2e -- --include-ignored
//! ```
//!
//! When the local machine has Docker but the test still wants to
//! be skipped (network restrictions, etc.), the fixture's
//! [`PebbleStartError::DockerUnavailable`] branch turns it into a
//! soft skip with an `eprintln!` rather than a panic.

#![cfg(feature = "acme")]

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use tempfile::{NamedTempFile, TempDir};
use vane_engine::acme::{AcmeStore, FsAcmeStore, ManagedCertRegistry};
use vane_testutil::acme::{Pebble, PebbleStartError};

/// Spawn Pebble or skip-with-message on Docker absence. A test
/// that returns `None` from this helper has logged its skip and
/// can early-return successfully.
async fn pebble_or_skip(test_name: &str) -> Option<Pebble> {
	// instant-acme uses rustls under the hood; it wants a
	// process-wide crypto provider installed before the first
	// `ServerConfig` / `ClientConfig` builder runs. The engine
	// installs aws-lc-rs at daemon boot via
	// `vane_engine::crypto::install_default_provider`; tests must
	// do the same.
	vane_engine::crypto::install_default_provider();

	match Pebble::start().await {
		Ok(p) => Some(p),
		Err(PebbleStartError::DockerUnavailable(msg)) => {
			eprintln!("skipping {test_name}: docker unavailable: {msg}");
			None
		}
		Err(e) => panic!("pebble start failed: {e}"),
	}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires docker"]
async fn http01_issues_cert_against_pebble_va_always_valid() {
	// Fixture wiring:
	//   - tmpdir-rooted FsAcmeStore
	//   - registry opened over that store
	//   - issue_http01_with_root pointed at Pebble's directory URL
	//     using the fetched root CA so instant-acme trusts the
	//     self-signed cert Pebble's HTTPS endpoint serves.
	let Some(pebble) = pebble_or_skip("http01_issues_cert_against_pebble_va_always_valid").await
	else {
		return;
	};

	let acme_dir = TempDir::new().expect("acme tmpdir");
	let store = Arc::new(FsAcmeStore::open(acme_dir.path()).expect("open store"));
	let registry = ManagedCertRegistry::open(Arc::clone(&store) as Arc<dyn AcmeStore>)
		.await
		.expect("open registry");

	// instant-acme's `Account::builder_with_root` reads the trust
	// root from a file path; write the in-memory PEM out to a
	// tempfile so the path is a real OS object the builder can
	// consume.
	// Pebble's HTTPS endpoint is signed by the `minica` root, not
	// the ACME-issuance root. `instant-acme` needs the minica root
	// to verify the HTTPS handshake during account creation.
	let mut root_pem_file = NamedTempFile::new().expect("root pem tmpfile");
	root_pem_file.write_all(&pebble.https_trust_root_pem).expect("write root pem");

	let sni = "test.example.com";
	let contact = vec!["mailto:ops@test.example.com".to_owned()];

	// Pebble in `PEBBLE_VA_ALWAYS_VALID=1` mode auto-passes the
	// http-01 challenge so the wall-clock for issuance is bounded
	// only by Pebble's poll cadence (sub-second). 30s gives
	// generous headroom for the fixture's container start tail.
	let issued = tokio::time::timeout(
		Duration::from_secs(30),
		registry.issue_http01_with_root(sni, &pebble.directory_url, &contact, root_pem_file.path()),
	)
	.await
	.expect("issuance within timeout")
	.expect("issuance ok");

	// Stored cert should round-trip through the registry's cache
	// AND be persisted on disk for restart-survival.
	assert!(!issued.leaf_pem.is_empty(), "leaf PEM must be populated");
	assert!(!issued.key_pem.is_empty(), "private key PEM must be populated");
	assert!(
		issued.not_after > std::time::SystemTime::now(),
		"issued cert must have a future not_after",
	);
	assert!(registry.cert_for(sni).is_some(), "cert must be cached by SNI");

	// Cross-process visibility: re-opening a fresh registry on the
	// same store directory must hydrate the cert from disk.
	let store_again = Arc::new(FsAcmeStore::open(acme_dir.path()).expect("re-open store"));
	let registry_again =
		ManagedCertRegistry::open(store_again as Arc<dyn AcmeStore>).await.expect("re-open registry");
	assert!(
		registry_again.cert_for(sni).is_some(),
		"cert must hydrate from disk on a fresh registry",
	);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires docker"]
async fn http01_issuance_is_idempotent_for_already_cached_sni() {
	// Two consecutive issuance attempts for the same SNI: the
	// second must short-circuit on the cache without round-tripping
	// the CA again. Confirms the cert-scope advisory lock + cache
	// fast path in `issue_http01_inner`.
	let Some(pebble) = pebble_or_skip("http01_issuance_is_idempotent_for_already_cached_sni").await
	else {
		return;
	};

	let acme_dir = TempDir::new().expect("acme tmpdir");
	let store = Arc::new(FsAcmeStore::open(acme_dir.path()).expect("open store"));
	let registry = ManagedCertRegistry::open(Arc::clone(&store) as Arc<dyn AcmeStore>)
		.await
		.expect("open registry");

	// Pebble's HTTPS endpoint is signed by the `minica` root, not
	// the ACME-issuance root. `instant-acme` needs the minica root
	// to verify the HTTPS handshake during account creation.
	let mut root_pem_file = NamedTempFile::new().expect("root pem tmpfile");
	root_pem_file.write_all(&pebble.https_trust_root_pem).expect("write root pem");

	let sni = "idempotent.example.com";
	let contact = vec!["mailto:ops@idempotent.example.com".to_owned()];

	let first = tokio::time::timeout(
		Duration::from_secs(30),
		registry.issue_http01_with_root(sni, &pebble.directory_url, &contact, root_pem_file.path()),
	)
	.await
	.expect("first issuance within timeout")
	.expect("first issuance ok");

	let second = tokio::time::timeout(
		Duration::from_secs(5),
		registry.issue_http01_with_root(sni, &pebble.directory_url, &contact, root_pem_file.path()),
	)
	.await
	.expect("second issuance must short-circuit fast")
	.expect("second issuance ok");

	assert_eq!(
		first.leaf_pem, second.leaf_pem,
		"second issuance must return the cached cert, not a fresh one",
	);
}
