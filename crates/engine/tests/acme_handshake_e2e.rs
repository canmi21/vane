//! End-to-end tests for the full ACME issuance → `ArcSwap` → rustls
//! handshake loop against [Pebble](https://github.com/letsencrypt/pebble).
//!
//! Stage 1's `acme_http01_e2e` covers the issuance round-trip; Stage
//! 3's `ManagedCertPopulator` + `FlowGraph::link` wiring is what
//! these tests verify: a cert that just landed in the registry's
//! cache surfaces through the populator into a `CertStore` and
//! satisfies a real rustls `ClientHello` against a bound TCP listener.
//!
//! Two scenarios:
//!
//! 1. `first_handshake_after_acme_issuance_uses_managed_cert` — the
//!    "did Stage 3's plumbing actually wire up?" smoke test.
//! 2. `managed_cert_persists_across_registry_reopen` — the persistence
//!    side: re-opening the registry on the same store directory
//!    re-hydrates the cached cert without triggering a fresh ACME
//!    order. Mirrors the `vaned restart` user story without spawning
//!    a real daemon process.
//!
//! Both `#[ignore = "requires docker"]`; soft-skip on Docker absence.

#![cfg(feature = "acme")]

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use tempfile::{NamedTempFile, TempDir};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vane_engine::acme::{AcmeStore, FsAcmeStore, ManagedCertPopulator, ManagedCertRegistry};
use vane_engine::tls::{CertPopulator, VaneCertResolver};
use vane_testutil::acme::{Pebble, PebbleStartError};

async fn pebble_or_skip(test_name: &str) -> Option<Pebble> {
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

/// Build a `tokio_rustls::rustls::ClientConfig` that trusts Pebble's
/// issuance root — the CA that signed the cert ACME delivered.
fn pebble_trusting_client_config(root_pem: &[u8]) -> rustls::ClientConfig {
	let mut roots = rustls::RootCertStore::empty();
	for cert in rustls_pemfile::certs(&mut std::io::Cursor::new(root_pem)) {
		let cert = cert.expect("parse pebble root pem");
		roots.add(cert).expect("add to root store");
	}
	rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires docker"]
async fn first_handshake_after_acme_issuance_uses_managed_cert() {
	let Some(pebble) = pebble_or_skip("first_handshake_after_acme_issuance_uses_managed_cert").await
	else {
		return;
	};

	// 1. Issue a cert through the registry. This re-uses the
	//    Stage-1 `issue_http01_with_root` path since Stage 3's
	//    contribution is the populator → resolver wiring on top of
	//    an already-populated registry.
	let acme_dir = TempDir::new().expect("acme tmpdir");
	let store = Arc::new(FsAcmeStore::open(acme_dir.path()).expect("open store"));
	let registry = ManagedCertRegistry::open(Arc::clone(&store) as Arc<dyn AcmeStore>)
		.await
		.expect("open registry");

	let mut https_root = NamedTempFile::new().expect("root pem tmpfile");
	https_root.write_all(&pebble.https_trust_root_pem).expect("write https root pem");

	let sni = "test.example.com";
	let contact = vec!["mailto:ops@test.example.com".to_owned()];
	let issued = tokio::time::timeout(
		Duration::from_secs(30),
		registry.issue_http01_with_root(sni, &pebble.directory_url, &contact, https_root.path()),
	)
	.await
	.expect("issuance within timeout")
	.expect("issuance ok");
	assert!(!issued.leaf_pem.is_empty(), "leaf PEM populated");
	let issued_leaf_der = parse_first_der(&issued.leaf_pem);

	// 2. Build the populator + resolver pipeline that
	//    `FlowGraph::link` would assemble for a `tls.managed`
	//    listener. We bypass `build_listener_server_config` and
	//    drive the public surface (`initial_store` →
	//    `VaneCertResolver`) directly — it's the same call
	//    sequence, just without the listener spec wrapper.
	let registry_arc = Arc::clone(&registry);
	let populator = ManagedCertPopulator::new(registry_arc, vec![sni.to_owned()]);
	let store = populator.initial_store().await.expect("initial_store");
	assert!(store.by_sni.contains_key(sni), "populator surfaced the issued cert");
	let arcswap = Arc::new(arc_swap::ArcSwap::from_pointee(store));
	let resolver = Arc::new(VaneCertResolver::new(arcswap));
	let server_config =
		rustls::ServerConfig::builder().with_no_client_auth().with_cert_resolver(resolver);
	let server_config = Arc::new(server_config);

	// 3. Bind an ephemeral TCP port + spawn an accept task that
	//    handles exactly one TLS handshake, sends a sentinel byte,
	//    and exits. The client connects to the same port with SNI
	//    set to `test.example.com` and Pebble's issuance root in
	//    its trust store.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let server_addr = listener.local_addr().expect("local_addr");
	let server_acceptor = tokio_rustls::TlsAcceptor::from(server_config);
	let server_task = tokio::spawn(async move {
		let (sock, _peer) = listener.accept().await.expect("accept");
		let mut tls = server_acceptor.accept(sock).await.expect("server handshake");
		tls.write_all(b"OK\n").await.expect("server write");
		tls.shutdown().await.ok();
	});

	let client_config = pebble_trusting_client_config(&pebble.root_ca_pem);
	let client_connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
	let server_name = ServerName::try_from(sni.to_owned()).expect("server_name");
	let tcp = tokio::net::TcpStream::connect(server_addr).await.expect("client connect");
	let mut tls = client_connector.connect(server_name, tcp).await.expect("client handshake");

	// 4. Confirm the negotiated cert chain matches what ACME just
	//    issued. The client side observes the leaf in
	//    `peer_certificates`; the first entry is the leaf.
	let (_, conn) = tls.get_ref();
	let peer_certs = conn.peer_certificates().expect("peer certs present");
	assert!(!peer_certs.is_empty());
	assert_eq!(
		peer_certs[0].as_ref(),
		issued_leaf_der.as_slice(),
		"handshake leaf must match the ACME-issued cert",
	);

	// 5. Drain the server's sentinel + cleanly close.
	let mut got = String::new();
	tls.read_to_string(&mut got).await.expect("client read");
	assert_eq!(got.trim(), "OK");
	tls.shutdown().await.ok();
	server_task.await.expect("server task join");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires docker"]
async fn managed_cert_persists_across_registry_reopen() {
	// Models the daemon-restart scenario: cert is in the on-disk
	// `FsAcmeStore`, daemon stops, daemon starts back up with the
	// same `VANE_ACME_DIR`, registry hydrates without triggering a
	// new ACME order. We simulate the restart by dropping the
	// first registry handle and opening a second one on the same
	// store path.
	let Some(pebble) = pebble_or_skip("managed_cert_persists_across_registry_reopen").await else {
		return;
	};

	let acme_dir = TempDir::new().expect("acme tmpdir");
	let store_path = acme_dir.path().to_path_buf();

	let mut https_root = NamedTempFile::new().expect("root pem tmpfile");
	https_root.write_all(&pebble.https_trust_root_pem).expect("write https root pem");

	let sni = "persist.test.example";
	let contact = vec!["mailto:ops@persist.test.example".to_owned()];

	// Issuance round 1: write cert to disk via the registry.
	let issued_leaf_der = {
		let store = Arc::new(FsAcmeStore::open(&store_path).expect("open store"));
		let registry = ManagedCertRegistry::open(Arc::clone(&store) as Arc<dyn AcmeStore>)
			.await
			.expect("open registry");
		let issued = tokio::time::timeout(
			Duration::from_secs(30),
			registry.issue_http01_with_root(sni, &pebble.directory_url, &contact, https_root.path()),
		)
		.await
		.expect("issuance within timeout")
		.expect("issuance ok");
		parse_first_der(&issued.leaf_pem)
		// `registry` and `store` drop here — simulates daemon
		// shutdown. The on-disk cert remains under `store_path`.
	};

	// Re-open. Hydrate must surface the prior cert without a fresh
	// ACME round-trip — the only way to confirm "no fresh order"
	// is to point at a directory_url that would refuse, but the
	// cleaner assertion is: registry.cert_for(sni) is hot
	// immediately after open() returns, before any issue_* call.
	let store2 = Arc::new(FsAcmeStore::open(&store_path).expect("re-open store"));
	let registry2 = ManagedCertRegistry::open(Arc::clone(&store2) as Arc<dyn AcmeStore>)
		.await
		.expect("re-open registry");
	let hot = registry2.cert_for(sni).expect("hydrated cert hot after reopen");
	let hot_der = parse_first_der(&hot.leaf_pem);
	assert_eq!(hot_der, issued_leaf_der, "hydrated cert matches the originally-issued DER");
}

/// Pull the first DER cert out of a PEM blob. Returns the raw bytes
/// — `peer_certificates()` returns DER, so byte-equality is the
/// cleanest "same cert" check.
fn parse_first_der(pem: &str) -> Vec<u8> {
	rustls_pemfile::certs(&mut std::io::Cursor::new(pem))
		.next()
		.expect("at least one cert in PEM")
		.expect("cert PEM parses")
		.as_ref()
		.to_vec()
}
