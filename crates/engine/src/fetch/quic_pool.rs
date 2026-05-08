//! Daemon-level upstream QUIC connection pool.
//!
//! Mirrors [`crate::fetch::client_cache`]'s lifetime contract — the
//! pool is constructed once at boot and lives until shutdown;
//! `FlowGraph` reload does not touch it. Entries are keyed by
//! [`QuicFingerprint`] and populated lazily on first dial.
//!
//! See `spec/crates/engine.md` § _Architecture: TCP / QUIC
//! separation_, § _Pool fingerprint_, § _Upstream pools_, and
//! § _Upstream pools_. The fingerprint shape differs from
//! [`crate::fetch::client_cache::ClientFingerprint`] in two ways:
//!
//! * `version` does not appear — QUIC is always H3 at the application
//!   layer, ALPN is always `[b"h3"]`, so there is nothing to vary on.
//! * `addr` does appear — each `QuicPool` entry binds its own ephemeral
//!   UDP socket and connects to one remote peer (the spec's
//!   `QuicPool socket model`); there is no per-authority connection
//!   pooling layer above this.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, LazyLock};

use bytes::Bytes;
use dashmap::DashMap;
use quinn::{ClientConfig, Endpoint};
use vane_core::{Error, UpstreamReason};

use crate::fetch::client_cache::TlsConfigFingerprint;

/// Identity of a pooled QUIC connection. Two `HttpProxyFetch`
/// instances share the same entry iff their `(addr, tls)` match
/// exactly.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct QuicFingerprint {
	/// Resolved upstream address. `quinn::Endpoint::connect` binds
	/// the connection to one peer, so the fingerprint must include
	/// the address — same TLS posture against two different addresses
	/// is two connections, not one shared.
	pub addr: SocketAddr,
	/// TLS posture (root CAs, verify mode, ALPN, mTLS slot, CRL slot).
	/// Reuses the TCP-side fingerprint shape so operator-facing
	/// `args.tls` parses once and feeds either the TCP or the QUIC
	/// pool — see `spec/crates/engine.md` § _Pool fingerprint_.
	/// ALPN is pinned to `[b"h3"]` for the H3 path; the factory sets
	/// it before fingerprinting.
	pub tls: TlsConfigFingerprint,
}

/// One pooled QUIC connection: the live `quinn::Endpoint` (kept
/// alive for the connection's lifetime), the H3 client driver task
/// joined into a tokio handle, and the `SendRequest` clone-source
/// that fetches use to issue requests.
///
/// `Drop` closes the endpoint and aborts the driver task — pool
/// removal is the cleanup signal. The driver's natural exit on
/// connection error also drops the entry from the pool via the
/// pool's monotonic-grow policy: a stale entry sits idle until
/// `quinn`'s connection idle timeout retires it from the inside;
/// the next dial finds the entry's `SendRequest` returning errors,
/// removes the entry, and re-dials. No active sweep — see
/// `spec/crates/engine.md` § _Upstream pools_.
pub struct QuicPoolEntry {
	/// `h3::client::SendRequest::clone()` is cheap (an internal
	/// `Arc` bump) and is the documented per-request handle source,
	/// so callers clone it on every `fetch` invocation. Stored as
	/// the bare value rather than `Mutex`-wrapped because `clone`
	/// takes `&self`.
	pub send_request: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
	/// Connection driver — h3 returns a `Connection` future that
	/// must be polled for the connection to make progress; the dial
	/// path spawns it as a tokio task and stashes the `JoinHandle`
	/// here so the pool entry's `Drop` can cancel cleanly.
	driver: tokio::task::JoinHandle<()>,
	/// Per-entry quinn `Endpoint` — owns the ephemeral UDP socket
	/// per `spec/crates/engine.md` § _Upstream pools_.
	/// Held on the entry so `Drop::drop` can close the endpoint
	/// before aborting the driver.
	endpoint: Endpoint,
	/// TLS server name supplied to the dial. Stored on the entry
	/// (rather than only on the fingerprint, which carries the
	/// resolved address but no hostname) so `snapshot()` can echo
	/// the operator's hostname for `get_upstreams`. Shared as
	/// `Arc<str>` because the same SNI is typically reused across
	/// pooled entries with the same TLS posture.
	pub sni: Arc<str>,
}

impl Drop for QuicPoolEntry {
	fn drop(&mut self) {
		// Best-effort: closing the endpoint signals the connection driver
		// to exit; aborting the JoinHandle prevents the task from
		// outliving the pool entry on connection-error paths.
		self.endpoint.close(0u32.into(), b"pool entry drop");
		self.driver.abort();
	}
}

// `quinn`'s connection idle timeout retires connections from the
// inside; manual eviction is exposed via `pool.drain` (see
// `drain_by_fingerprint_id`). Cache grows monotonically across reload
// cycles otherwise (see `spec/crates/engine.md`
// § _Upstream pools_).
static QUIC_POOL: LazyLock<DashMap<QuicFingerprint, Arc<QuicPoolEntry>>> =
	LazyLock::new(DashMap::new);

/// Look up the pooled entry for `fp`, returning `None` on miss.
/// [`get_or_dial`] is the standard accessor; this read-only variant
/// is exposed for the dispatch path's "fast cache hit" branch and
/// for tests that assert pool population without triggering a dial.
#[must_use]
pub fn get(fp: &QuicFingerprint) -> Option<Arc<QuicPoolEntry>> {
	QUIC_POOL.get(fp).map(|r| Arc::clone(&r))
}

/// Remove the entry for `fp` from the pool. Used by the dispatch
/// path on a connection-error round-trip so the next request re-dials
/// rather than reusing a dead entry.
pub fn evict(fp: &QuicFingerprint) {
	QUIC_POOL.remove(fp);
}

/// Acquire the pooled entry for `fp`, dialing on miss. The dial
/// builds a per-entry `quinn::Endpoint` bound to a fresh ephemeral
/// UDP socket (per `spec/crates/engine.md` § _`QuicPool` socket
/// model_), runs the QUIC handshake against `fp.addr` with `sni` as
/// the TLS server name, then negotiates h3 and spawns the connection
/// driver as a background tokio task.
///
/// `rustls_cfg` carries the TLS posture that `fp.tls` fingerprints —
/// callers build it once at factory time and reuse the `Arc` for
/// every dial. ALPN must already be `[b"h3"]`; the helper wraps the
/// config into `quinn::ClientConfig` without further mutation.
///
/// Race-tolerant: two concurrent dialers for the same fingerprint
/// may both reach `dial_new`; the loser's entry is dropped (closing
/// its endpoint) and the winner's is returned to both. The wasted
/// dial is the only cost — `quinn::Endpoint::client` binds a UDP
/// socket but the handshake's pacing makes spurious dials rare in
/// practice.
///
/// On dial failure (DNS, connect, TLS handshake, h3 negotiation) the
/// pool stays empty for `fp` — the next call re-dials. Errors
/// surface as [`Error::upstream`] with a reason matching the failure
/// stage (`Unreachable` for transport, `TlsHandshake` for crypto).
///
/// # Errors
///
/// Returns [`Error::upstream`] for any failure in the dial chain
/// (UDP bind, quinn config build, connect, h3 negotiation). The
/// inner source carries the original error message for tracing.
pub async fn get_or_dial(
	fp: QuicFingerprint,
	sni: &str,
	rustls_cfg: Arc<rustls::ClientConfig>,
) -> Result<Arc<QuicPoolEntry>, Error> {
	if let Some(existing) = get(&fp) {
		return Ok(existing);
	}
	let entry = dial_new(&fp, sni, rustls_cfg).await?;
	let inserted = QUIC_POOL.entry(fp).or_insert(entry);
	Ok(Arc::clone(&inserted))
}

/// Build one fresh QUIC connection + h3 client. Separate from
/// [`get_or_dial`] so the cache-miss path has a single self-contained
/// failure surface — `?` early-returns abort the dial and leave the
/// pool unchanged.
async fn dial_new(
	fp: &QuicFingerprint,
	sni: &str,
	rustls_cfg: Arc<rustls::ClientConfig>,
) -> Result<Arc<QuicPoolEntry>, Error> {
	// `quinn::crypto::rustls::QuicClientConfig::try_from` consumes the
	// rustls config — clone the inner so the cached `Arc` stays alive
	// for sibling dials. Upstream mTLS rides on the rustls
	// `client_auth_cert_resolver` already installed by
	// `build_client_config_with_crls`; quinn carries the entire rustls
	// `ClientConfig` through (`QuicClientConfig` wraps `Arc<ClientConfig>`
	// in `quinn-proto`'s `TryFrom`), so no additional QUIC-side wiring
	// is required.
	let inner_rustls: rustls::ClientConfig = (*rustls_cfg).clone();
	let quic_crypto =
		quinn::crypto::rustls::QuicClientConfig::try_from(inner_rustls).map_err(|e| {
			Error::upstream(UpstreamReason::TlsHandshake)
				.with_source(std::io::Error::other(format!("quic client config: {e}")))
		})?;
	let client_cfg = ClientConfig::new(Arc::new(quic_crypto));

	// Bind ephemeral UDP. Match the address family of the target so
	// `quinn::Endpoint::connect` doesn't have to translate; this also
	// avoids the unwieldy v4-mapped-v6 path on macOS.
	let bind_addr: SocketAddr = if fp.addr.is_ipv6() {
		SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
	} else {
		SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
	};
	let mut endpoint = Endpoint::client(bind_addr).map_err(|e| {
		Error::upstream(UpstreamReason::Unreachable)
			.with_source(std::io::Error::other(format!("quinn client endpoint bind: {e}")))
	})?;
	endpoint.set_default_client_config(client_cfg);

	let connecting = endpoint.connect(fp.addr, sni).map_err(|e| {
		Error::upstream(UpstreamReason::TlsHandshake).with_source(std::io::Error::new(
			std::io::ErrorKind::InvalidInput,
			format!("quinn connect call: {e}"),
		))
	})?;
	let quic_conn = connecting.await.map_err(|e| {
		Error::upstream(UpstreamReason::Unreachable)
			.with_source(std::io::Error::other(format!("quic handshake: {e}")))
	})?;

	let h3_quic = h3_quinn::Connection::new(quic_conn);
	let (mut driver, send_request) = h3::client::new(h3_quic).await.map_err(|e| {
		Error::upstream(UpstreamReason::Unreachable)
			.with_source(std::io::Error::other(format!("h3 client setup: {e}")))
	})?;
	let driver = tokio::spawn(async move {
		// `poll_close` resolves with the terminal connection error once
		// the connection is fully drained. Logging the error keeps the
		// driver visible in tracing without surfacing it on the dispatch
		// path (the next request will fail naturally if the connection
		// is dead).
		let err = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
		tracing::debug!(?err, "h3 upstream connection driver exited");
	});

	Ok(Arc::new(QuicPoolEntry { send_request, driver, endpoint, sni: Arc::from(sni) }))
}

/// Number of pooled entries. Test-only: integration tests assert
/// pool sharing by counting entries before and after a sequence of
/// dials. Production code does not consult cache cardinality.
#[doc(hidden)]
#[must_use]
pub fn cache_len() -> usize {
	QUIC_POOL.len()
}

/// Read-only summary of one pooled QUIC connection. Surfaced via the
/// `get_upstreams` mgmt verb. ALPN is bytes on the fingerprint;
/// decoded lossily here for the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PooledQuicSummary {
	pub remote_addr: String,
	pub sni: String,
	pub alpn: Vec<String>,
	/// Stable identifier (16-char hex) the operator uses to address
	/// this entry in `pool.drain`.
	pub fingerprint_id: String,
}

/// Hash a `QuicFingerprint` into a stable 16-char hex string. Same
/// shape as [`crate::fetch::client_cache::fingerprint_id`].
#[must_use]
pub fn fingerprint_id(fp: &QuicFingerprint) -> String {
	use std::hash::{Hash as _, Hasher as _};
	let mut h = std::collections::hash_map::DefaultHasher::new();
	fp.hash(&mut h);
	format!("{:016x}", h.finish())
}

/// Snapshot every pooled QUIC connection. Read-only: never inserts,
/// never dials. The `sni` field reports the hostname supplied to
/// `get_or_dial` (stored on [`QuicPoolEntry`] — the fingerprint
/// itself only carries the resolved address).
#[must_use]
pub fn snapshot() -> Vec<PooledQuicSummary> {
	QUIC_POOL
		.iter()
		.map(|entry| {
			let fp = entry.key();
			let alpn =
				fp.tls.alpn_protocols.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect();
			PooledQuicSummary {
				remote_addr: fp.addr.to_string(),
				sni: entry.value().sni.as_ref().to_owned(),
				alpn,
				fingerprint_id: fingerprint_id(fp),
			}
		})
		.collect()
}

/// Remove pool entries whose `fingerprint_id` matches `id`. Returns
/// the number of entries actually removed. Live `Arc<QuicPoolEntry>`
/// references are unaffected — the operator's drain only changes
/// future cache lookups.
#[must_use]
pub fn drain_by_fingerprint_id(id: &str) -> usize {
	let to_remove: Vec<QuicFingerprint> = QUIC_POOL
		.iter()
		.filter_map(
			|entry| {
				if fingerprint_id(entry.key()) == id { Some(entry.key().clone()) } else { None }
			},
		)
		.collect();
	let mut removed = 0_usize;
	for fp in to_remove {
		if QUIC_POOL.remove(&fp).is_some() {
			removed += 1;
		}
	}
	removed
}

/// Empty the pool. Test-only — integration tests call this between
/// scenarios to keep entry-count assertions independent. Each dropped
/// entry's `Drop` closes its endpoint and aborts its driver, so the
/// background quinn tasks shut down cleanly.
#[doc(hidden)]
pub fn clear_for_test() {
	QUIC_POOL.clear();
}

#[cfg(test)]
mod tests {
	use std::net::{IpAddr, Ipv4Addr};

	use super::*;
	use crate::fetch::client_cache::{RootCaSource, VerifyMode};

	fn sample_fp(port: u16) -> QuicFingerprint {
		QuicFingerprint {
			addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
			tls: TlsConfigFingerprint {
				root_ca: RootCaSource::Skip,
				client_cert_hash: None,
				crl_sources: Vec::new(),
				verify_mode: VerifyMode::Skip,
				alpn_protocols: vec![b"h3".to_vec()],
			},
		}
	}

	#[test]
	fn fingerprint_eq_same_inputs() {
		let a = sample_fp(443);
		let b = sample_fp(443);
		assert_eq!(a, b);
	}

	#[test]
	fn fingerprint_neq_different_addr() {
		assert_ne!(sample_fp(443), sample_fp(8443));
	}

	#[test]
	fn fingerprint_neq_secure_vs_insecure() {
		let mut a = sample_fp(443);
		a.tls.verify_mode = VerifyMode::Full;
		a.tls.root_ca = RootCaSource::System;
		let b = sample_fp(443);
		assert_ne!(a, b, "verify mode + root CA source must each contribute to the hash");
	}

	#[test]
	fn get_returns_none_on_empty_pool() {
		clear_for_test();
		assert!(get(&sample_fp(9999)).is_none());
	}

	#[test]
	fn snapshot_is_empty_on_clean_pool() {
		clear_for_test();
		assert!(snapshot().is_empty(), "fresh pool snapshot must be empty");
	}
}
