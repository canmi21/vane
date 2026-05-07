//! Pebble fixture — runs Let's Encrypt's official ACME test server
//! in a local Docker container via `testcontainers`. Engine
//! integration tests for the HTTP-01 issuance flow opt in via the
//! `acme` feature.
//!
//! ## Validation modes
//!
//! Stage 1 ships the fixture in `PEBBLE_VA_ALWAYS_VALID=1` mode:
//! Pebble walks the full RFC 8555 order workflow but skips the
//! HTTP-01 challenge fetch. That covers the
//! `instant-acme` round-trip (account create → order → finalize →
//! cert download) end to end without needing
//! Docker-container-to-host network passthrough — which is
//! platform-specific (`host.docker.internal` on macOS, an
//! `--add-host=host-gateway` on Linux). The challenge-fetch path
//! is unit-tested in `crates/engine/src/fetch/acme_challenge.rs`.
//!
//! A future fixture mode that flips `PEBBLE_VA_ALWAYS_VALID=0`
//! and routes Pebble's validator to a host port lands when the
//! Stage 3 renewal scheduler arrives.
//!
//! ## Skip-if-no-docker
//!
//! [`Pebble::start`] returns
//! [`PebbleStartError::DockerUnavailable`] when the local Docker
//! daemon can't be reached. Tests should treat this as "skip" so
//! `cargo nextest run --workspace` on a CI / dev machine without
//! Docker installed still passes.

#![cfg(feature = "acme")]

use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use thiserror::Error;
use tracing::info;

// Pebble is published on GHCR; the legacy Docker Hub mirror was
// deprecated. The repository only ships `latest` (no semver tags
// at the time of writing); pin tag-by-digest is overkill for a
// test harness, so accept some tag drift in exchange for the
// project's up-to-date validator behaviour.
const PEBBLE_IMAGE: &str = "ghcr.io/letsencrypt/pebble";
const PEBBLE_TAG: &str = "latest";
const ACME_PORT: u16 = 14000;
const MGMT_PORT: u16 = 15000;

/// Live Pebble instance. Drop the value to stop the container.
pub struct Pebble {
	pub directory_url: String,
	pub management_url: String,
	/// PEM of the CA whose root is implicit in Pebble's
	/// HTTPS-endpoint cert chain — fetched via `/roots/0`. Not
	/// the same as [`Self::https_trust_root_pem`].
	pub root_ca_pem: Vec<u8>,
	/// PEM of the `minica` root that signs Pebble's *own* HTTPS
	/// endpoint cert (the one served on the management + ACME
	/// ports). Distinct from `root_ca_pem` because the cert
	/// Pebble issues to ACME clients chains through a different
	/// hierarchy than Pebble's own server cert. `instant-acme`'s
	/// `Account::builder_with_root` needs this one to verify the
	/// HTTPS endpoint during account creation.
	pub https_trust_root_pem: Vec<u8>,
	_container: ContainerAsync<GenericImage>,
}

#[derive(Debug, Error)]
pub enum PebbleStartError {
	#[error("docker unavailable: {0}")]
	DockerUnavailable(String),
	#[error("pebble container failed to come up: {0}")]
	ContainerStartup(String),
	#[error("could not fetch pebble root CA: {0}")]
	RootCaFetch(String),
}

impl Pebble {
	/// Boot a Pebble container in `VA_ALWAYS_VALID=1` mode and
	/// fetch its self-signed root CA. Returns
	/// [`PebbleStartError::DockerUnavailable`] when the local
	/// Docker daemon is unreachable so callers can treat that as
	/// "skip the test".
	///
	/// Defaults:
	/// - `PEBBLE_VA_NOSLEEP=1` — skip Pebble's artificial 0..15s
	///   per-validation sleep.
	/// - `PEBBLE_VA_ALWAYS_VALID=1` — auto-pass the challenge.
	/// - `PEBBLE_AUTHZREUSE=100` — reuse authorisations across
	///   orders within the daemon lifetime.
	///
	/// # Errors
	///
	/// As above. Returns owned strings rather than borrowed
	/// references so tests can hold the URLs across `await`s.
	pub async fn start() -> Result<Self, PebbleStartError> {
		// Pebble logs the "ACME directory available at" line to stdout
		// after binding both the directory and management endpoints,
		// so it's the right ready signal for the testcontainers
		// readiness probe.
		let image = GenericImage::new(PEBBLE_IMAGE, PEBBLE_TAG)
			.with_exposed_port(ACME_PORT.tcp())
			.with_exposed_port(MGMT_PORT.tcp())
			.with_wait_for(WaitFor::message_on_stdout("ACME directory available at"));

		let container = image
			.with_env_var("PEBBLE_VA_ALWAYS_VALID", "1")
			.with_env_var("PEBBLE_VA_NOSLEEP", "1")
			.with_env_var("PEBBLE_AUTHZREUSE", "100")
			.start()
			.await
			.map_err(|e| classify_container_error(&e))?;

		// Force IPv4 in the URLs — testcontainers' `get_host` may
		// return "localhost", which resolves to `::1` first on
		// macOS. instant-acme's hyper-rustls client doesn't always
		// fall back to IPv4 quickly, so pin 127.0.0.1 explicitly.
		// IPv4-only is fine here because we're talking to a
		// loopback Docker port mapping.
		let acme_port = container
			.get_host_port_ipv4(ACME_PORT)
			.await
			.map_err(|e| PebbleStartError::ContainerStartup(format!("acme port: {e}")))?;
		let mgmt_port = container
			.get_host_port_ipv4(MGMT_PORT)
			.await
			.map_err(|e| PebbleStartError::ContainerStartup(format!("mgmt port: {e}")))?;
		let directory_url = format!("https://127.0.0.1:{acme_port}/dir");
		let management_url = format!("https://127.0.0.1:{mgmt_port}");

		let root_ca_pem = fetch_root_ca(&management_url).await?;
		let https_trust_root_pem = extract_minica_root(container.id()).await?;

		info!(
			target: "vane::testutil::acme",
			%directory_url,
			%management_url,
			"pebble started",
		);
		Ok(Self {
			directory_url,
			management_url,
			root_ca_pem,
			https_trust_root_pem,
			_container: container,
		})
	}
}

/// Pebble's HTTPS endpoint serves a leaf signed by `minica root
/// ca <id>`, NOT by the ACME-issuance root that `/roots/0`
/// returns. To make `instant-acme` trust the HTTPS endpoint, we
/// need the minica root from inside the container at
/// `/test/certs/pebble.minica.pem`.
///
/// The Pebble image is distroless / scratch-based — no shell, no
/// `cat`. testcontainers-rs's `exec` API requires shell utilities
/// in the target image. The simplest reliable extraction is
/// shelling out to `docker cp` (which the Docker daemon handles
/// kernel-side without needing any binary in the container).
async fn extract_minica_root(container_id: &str) -> Result<Vec<u8>, PebbleStartError> {
	use std::process::Stdio;
	let output = tokio::process::Command::new("docker")
		.arg("cp")
		.arg(format!("{container_id}:/test/certs/pebble.minica.pem"))
		.arg("-")
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.output()
		.await
		.map_err(|e| PebbleStartError::RootCaFetch(format!("spawn docker cp: {e}")))?;
	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).to_string();
		return Err(PebbleStartError::RootCaFetch(format!(
			"docker cp failed (status {:?}): {stderr}",
			output.status.code()
		)));
	}
	// `docker cp ... -` writes a tar archive to stdout. The PEM
	// is the only file we care about; pull it out by skipping the
	// 512-byte tar header and reading the file body up to the
	// next 512-byte boundary.
	extract_pem_from_tar(&output.stdout).ok_or_else(|| {
		PebbleStartError::RootCaFetch("docker cp produced no PEM body in tar archive".to_owned())
	})
}

fn extract_pem_from_tar(tar_bytes: &[u8]) -> Option<Vec<u8>> {
	// Minimal POSIX-tar reader: skip the header, read `size` bytes
	// of the body. We don't need full tar semantics because the
	// archive `docker cp` writes for a single regular file is
	// exactly `<header><body><padding>`.
	if tar_bytes.len() < 512 {
		return None;
	}
	// `size` is at offset 124, 12 octal-ASCII bytes, NUL-padded.
	let size_field = &tar_bytes[124..136];
	let size_str = std::str::from_utf8(size_field).ok()?.trim_end_matches('\0').trim();
	let size = u64::from_str_radix(size_str, 8).ok()?;
	let body_start = 512;
	let body_end = body_start + usize::try_from(size).ok()?;
	if tar_bytes.len() < body_end {
		return None;
	}
	Some(tar_bytes[body_start..body_end].to_vec())
}

fn classify_container_error(err: &testcontainers::TestcontainersError) -> PebbleStartError {
	let msg = err.to_string();
	// testcontainers wraps multiple failure modes inside a single
	// error type. The strings we recognise as docker-unavailable
	// are the ones bollard surfaces when the Docker daemon socket
	// doesn't answer (no daemon installed, daemon stopped, missing
	// permission to /var/run/docker.sock).
	let lower = msg.to_lowercase();
	if lower.contains("docker") && (lower.contains("connect") || lower.contains("permission"))
		|| lower.contains("no such file or directory") && lower.contains("docker")
	{
		PebbleStartError::DockerUnavailable(msg)
	} else {
		PebbleStartError::ContainerStartup(msg)
	}
}

async fn fetch_root_ca(management_url: &str) -> Result<Vec<u8>, PebbleStartError> {
	// Pebble's self-signed CA is at /roots/0; we accept its
	// management endpoint's TLS cert without verification because
	// it's self-signed by definition. The fetched root CA is what
	// instant-acme uses as a trusted root for Pebble's directory
	// endpoint.
	let client = reqwest::ClientBuilder::new()
		.danger_accept_invalid_certs(true)
		.timeout(Duration::from_secs(10))
		.build()
		.map_err(|e| PebbleStartError::RootCaFetch(format!("client build: {e}")))?;
	let url = format!("{management_url}/roots/0");
	let resp = client
		.get(&url)
		.send()
		.await
		.map_err(|e| PebbleStartError::RootCaFetch(format!("GET {url}: {e}")))?;
	if !resp.status().is_success() {
		return Err(PebbleStartError::RootCaFetch(format!("GET {url}: status {}", resp.status())));
	}
	let bytes =
		resp.bytes().await.map_err(|e| PebbleStartError::RootCaFetch(format!("read body: {e}")))?;
	Ok(bytes.to_vec())
}
