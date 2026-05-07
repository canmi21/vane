//! End-to-end tests for DNS-01 ACME issuance against
//! [Pebble](https://github.com/letsencrypt/pebble) with the
//! validation authority pointed at the in-process
//! [`vane_testutil::acme::MockDns`] server.
//!
//! Unlike the http-01 e2e (which runs Pebble with
//! `PEBBLE_VA_ALWAYS_VALID=1` to skip challenge fetches), DNS-01
//! tests *do* exercise the validator path: Pebble queries
//! `host.docker.internal:<MockDns port>` for the
//! `_acme-challenge.<sni>` TXT record, the `MockDnsProvider`
//! writes match the ACME server's expected value, and the cert
//! is issued only when the TXT actually propagates.
//!
//! Tests are `#[ignore = "requires docker"]` and soft-skip when
//! Docker is unavailable so the default `cargo nextest run
//! --workspace` stays passing on machines without Docker.

#![cfg(feature = "acme")]

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use tempfile::{NamedTempFile, TempDir};
use vane_engine::acme::{AcmeStore, FsAcmeStore, ManagedCertRegistry};
use vane_testutil::acme::{MockDns, Pebble, PebbleStartError};

/// Spawn the [`MockDns`] + [`Pebble`] pair, or skip-with-message
/// on Docker / bind absence. Returns owned values so the test
/// functions can pin them to the test's tokio runtime lifetime.
async fn fixtures_or_skip(test_name: &str) -> Option<(MockDns, Pebble)> {
	vane_engine::crypto::install_default_provider();

	let mock = match MockDns::start().await {
		Ok(m) => m,
		Err(e) => panic!("mock dns start failed: {e}"),
	};

	let pebble = match Pebble::start_with_dns_resolver(mock.addr()).await {
		Ok(p) => p,
		Err(PebbleStartError::DockerUnavailable(msg)) => {
			eprintln!("skipping {test_name}: docker unavailable: {msg}");
			return None;
		}
		Err(e) => panic!("pebble start failed: {e}"),
	};
	Some((mock, pebble))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires docker"]
async fn dns01_issues_cert_against_pebble_with_mock_dns() {
	let Some((mock, pebble)) =
		fixtures_or_skip("dns01_issues_cert_against_pebble_with_mock_dns").await
	else {
		return;
	};

	let acme_dir = TempDir::new().expect("acme tmpdir");
	let store = Arc::new(FsAcmeStore::open(acme_dir.path()).expect("open store"));
	let registry = ManagedCertRegistry::open(Arc::clone(&store) as Arc<dyn AcmeStore>)
		.await
		.expect("open registry");

	let mut root_pem_file = NamedTempFile::new().expect("root pem tmpfile");
	root_pem_file.write_all(&pebble.https_trust_root_pem).expect("write root pem");

	let sni = "test.example";
	let contact = vec!["mailto:ops@test.example".to_owned()];
	let dns = mock.provider();

	let issued = tokio::time::timeout(
		Duration::from_mins(1),
		registry.issue_dns01_with_root(sni, &pebble.directory_url, &contact, root_pem_file.path(), dns),
	)
	.await
	.expect("issuance within timeout")
	.expect("issuance ok");

	assert!(!issued.leaf_pem.is_empty(), "leaf PEM populated");
	assert!(!issued.key_pem.is_empty(), "key PEM populated");
	assert!(issued.not_after > std::time::SystemTime::now(), "not_after must be in the future");
	assert!(registry.cert_for(sni).is_some(), "cert cached by SNI");

	// Mock store should be empty after issuance — the cleanup_now
	// path runs synchronously on success, so the TXT record we
	// set during the challenge is gone by function return.
	assert!(
		mock.txt_records("_acme-challenge.test.example").is_empty(),
		"successful issuance must clean up its TXT record",
	);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires docker"]
async fn dns01_issues_wildcard_san_against_pebble() {
	// RFC 8555 only allows DNS-01 for wildcard SANs; this is
	// the one ACME flow that *must* go through DNS-01.
	let Some((mock, pebble)) = fixtures_or_skip("dns01_issues_wildcard_san_against_pebble").await
	else {
		return;
	};

	let acme_dir = TempDir::new().expect("acme tmpdir");
	let store = Arc::new(FsAcmeStore::open(acme_dir.path()).expect("open store"));
	let registry = ManagedCertRegistry::open(Arc::clone(&store) as Arc<dyn AcmeStore>)
		.await
		.expect("open registry");

	let mut root_pem_file = NamedTempFile::new().expect("root pem tmpfile");
	root_pem_file.write_all(&pebble.https_trust_root_pem).expect("write root pem");

	// Pebble's order accepts the wildcard literally; the authz
	// it returns has the *base* identifier (no `*.` prefix). The
	// TXT name we end up setting is `_acme-challenge.example` —
	// not `_acme-challenge.*.example`, which would be invalid DNS.
	let sni = "*.wild.example";
	let contact = vec!["mailto:ops@wild.example".to_owned()];
	let dns = mock.provider();

	let issued = tokio::time::timeout(
		Duration::from_mins(1),
		registry.issue_dns01_with_root(sni, &pebble.directory_url, &contact, root_pem_file.path(), dns),
	)
	.await
	.expect("issuance within timeout")
	.expect("issuance ok");

	assert!(!issued.leaf_pem.is_empty());
	// Cache must be keyed by the wildcard SNI verbatim — that's
	// what the cert resolver looks up at handshake time.
	assert!(registry.cert_for(sni).is_some(), "wildcard cached by SNI");
}
