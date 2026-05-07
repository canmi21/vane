//! Daemon-level upstream `Client` cache.
//!
//! Keys `hyper_util::client::legacy::Client` instances by
//! `(version, Option<TlsConfigFingerprint>)` so two `HttpProxyFetch`
//! instances with the same TLS posture share a single `Arc<Client>`
//! — and therefore a single per-authority connection pool. Cleartext
//! upstreams use `tls = None` and still share by `version`.
//!
//! See `spec/crates/engine-tls.md` § _Client cache: fingerprint and
//! reuse_ for the authoritative semantics; `spec/crates/engine.md`
//! § _Pool fingerprint_ for the design rationale.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use vane_core::Body;

use crate::fetch::dns::DnsConfig;
use crate::fetch::dns::HickoryDnsResolver;
use crate::fetch::http_proxy::UpstreamVersion;

pub type ProxyClient = Client<HttpsConnector<HttpConnector<HickoryDnsResolver>>, Body>;

/// Trust-root posture. Distinct variants get distinct fingerprints so
/// pools never share across security postures — an
/// `insecure_skip_verify` config and a system-roots config must never
/// land in the same cache slot.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum RootCaSource {
	/// `rustls-native-certs` system trust store. Constant tag.
	System,
	/// Operator-supplied root CA bundle (PEM).
	Bundle(PathBuf),
	/// `insecure_skip_verify: true` — `NoVerify` accepts any cert.
	Skip,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum VerifyMode {
	Full,
	Skip,
}

/// CRL source identity participates in the fingerprint by *string*,
/// not by fetched bytes. See `spec/crates/engine-tls.md` § _Client cache_ rationale —
/// hashing CRL content would force a new client on every refresh,
/// defeating the cache.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum CrlSource {
	File(PathBuf),
	Url(String),
}

/// Fingerprint of one TLS posture. CRL participates by *source
/// identity*, not by fetched content (see spec note). `client_cert_hash`
/// is `Some([u8; 32])` (SHA-256 of the leaf cert DER) when upstream
/// mTLS is configured, `None` otherwise; cleartext upstreams keep
/// `tls: None` on `ClientFingerprint` and never reach this struct.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct TlsConfigFingerprint {
	pub root_ca: RootCaSource,
	pub client_cert_hash: Option<[u8; 32]>,
	pub crl_sources: Vec<CrlSource>,
	pub verify_mode: VerifyMode,
	pub alpn_protocols: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct ClientFingerprint {
	pub version: UpstreamVersion,
	/// `None` on cleartext upstream.
	pub tls: Option<TlsConfigFingerprint>,
	/// DNS posture. `System` reads `/etc/resolv.conf` (default);
	/// `Custom` pins resolution at the listed nameservers, in order.
	/// Order is load-bearing — see [`DnsConfig`] doc.
	pub dns: DnsConfig,
}

// Cache grows monotonically across reload cycles; manual eviction is
// available via `pool.drain <fingerprint_id>` (see
// `crate::fetch::client_cache::drain_by_fingerprint_id`). Live
// `Arc<Client>` references survive eviction — operators removing a
// fingerprint affect only future cache lookups.
static CLIENT_CACHE: LazyLock<DashMap<ClientFingerprint, Arc<ProxyClient>>> =
	LazyLock::new(DashMap::new);

/// Get the cached `Arc<Client>` for `fp`, or build it via `build`
/// and insert. Returns the cached value's `Arc` clone — cheap.
///
/// Race-tolerant: under contention multiple threads may evaluate
/// `build` concurrently for the same fingerprint, but only one
/// `Arc` survives in the map. `Arc<Client>` clones are refcount
/// bumps so the wasted build is the only cost; `legacy::Client`
/// itself is internally `Arc`-y so spurious construction does not
/// bind any port or open any socket.
pub fn get_or_build(
	fp: ClientFingerprint,
	build: impl FnOnce() -> ProxyClient,
) -> Arc<ProxyClient> {
	if let Some(existing) = CLIENT_CACHE.get(&fp) {
		return Arc::clone(&existing);
	}
	let arc = Arc::new(build());
	let entry = CLIENT_CACHE.entry(fp).or_insert(arc);
	Arc::clone(&entry)
}

/// Number of cached clients. Test-only: integration tests use this
/// to assert the cache is sized correctly after a sequence of
/// factory calls. Production code does not consult cache cardinality.
#[doc(hidden)]
#[must_use]
pub fn cache_len() -> usize {
	CLIENT_CACHE.len()
}

/// Read-only summary of one cached client. Carries enough of the
/// fingerprint to be useful in operator-facing observability without
/// echoing PEM-bundle paths or other filesystem detail.
///
/// Surfaced via the `get_upstreams` mgmt verb; the daemon translates
/// these into wire-shape entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedClientSummary {
	pub version: &'static str,
	pub scheme: &'static str,
	pub root_ca: &'static str,
	pub verify_mode: &'static str,
	pub alpn: Vec<String>,
	pub dns: &'static str,
	/// Stable identifier for this fingerprint within the running
	/// process (16-char hex of `DefaultHasher`). Operators pass it back
	/// to `pool.drain` to remove this entry; survives across calls and
	/// reloads as long as the underlying fingerprint contents don't
	/// change.
	pub fingerprint_id: String,
}

/// Hash a `ClientFingerprint` into a stable 16-char hex string. Same
/// inputs yield the same ID within a process; `DefaultHasher::new`
/// uses fixed seeds so the value is stable for the process lifetime.
#[must_use]
pub fn fingerprint_id(fp: &ClientFingerprint) -> String {
	use std::hash::{Hash as _, Hasher as _};
	let mut h = std::collections::hash_map::DefaultHasher::new();
	fp.hash(&mut h);
	format!("{:016x}", h.finish())
}

/// Snapshot every cached client. Allocation-free for the lookup,
/// allocates once per entry to lossy-decode ALPN bytes. Read-only:
/// never inserts, never builds.
#[must_use]
pub fn snapshot() -> Vec<CachedClientSummary> {
	CLIENT_CACHE
		.iter()
		.map(|entry| {
			let fp = entry.key();
			let version = match fp.version {
				UpstreamVersion::Auto => "auto",
				UpstreamVersion::Http1 => "h1",
				UpstreamVersion::Http2 => "h2",
				#[cfg(feature = "h3")]
				UpstreamVersion::Http3 => "h3",
			};
			let (scheme, root_ca, verify_mode, alpn) = match &fp.tls {
				None => ("http", "none", "none", Vec::new()),
				Some(tls) => {
					let root_ca = match tls.root_ca {
						RootCaSource::System => "system",
						RootCaSource::Bundle(_) => "bundle",
						RootCaSource::Skip => "insecure-skip",
					};
					let verify_mode = match tls.verify_mode {
						VerifyMode::Full => "full",
						VerifyMode::Skip => "skip",
					};
					let alpn =
						tls.alpn_protocols.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect();
					("https", root_ca, verify_mode, alpn)
				}
			};
			let dns = match fp.dns {
				DnsConfig::System => "system",
				DnsConfig::Custom(_) => "custom",
			};
			let fingerprint_id = fingerprint_id(fp);
			CachedClientSummary { version, scheme, root_ca, verify_mode, alpn, dns, fingerprint_id }
		})
		.collect()
}

/// Empty the cache. Test-only — integration tests call this between
/// scenarios to keep accept-counter assertions independent. Calling
/// it from production code would orphan in-flight `Arc<Client>`
/// handles (which is fine but pointless).
#[doc(hidden)]
pub fn clear_cache_for_test() {
	CLIENT_CACHE.clear();
}

/// Remove cache entries whose `fingerprint_id` matches `id`. Returns
/// the number of entries actually removed (typically 0 or 1).
///
/// Live `Arc<Client>` references already issued by `get_or_build` are
/// **not** invalidated — `DashMap` removal drops the cache's strong
/// reference, but in-flight requests still hold their own `Arc`
/// clone. New requests for the same fingerprint go through
/// `get_or_build`'s build path and insert a fresh entry. This matches
/// the spec's "operator drain affects only future lookups" contract.
#[must_use]
pub fn drain_by_fingerprint_id(id: &str) -> usize {
	let to_remove: Vec<ClientFingerprint> = CLIENT_CACHE
		.iter()
		.filter_map(
			|entry| {
				if fingerprint_id(entry.key()) == id { Some(entry.key().clone()) } else { None }
			},
		)
		.collect();
	let mut removed = 0_usize;
	for fp in to_remove {
		if CLIENT_CACHE.remove(&fp).is_some() {
			removed += 1;
		}
	}
	removed
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicUsize, Ordering};

	use super::*;
	use crate::fetch::http_proxy::UpstreamVersion;

	fn sample_tls_fp(insecure: bool, alpn: Vec<Vec<u8>>) -> TlsConfigFingerprint {
		TlsConfigFingerprint {
			root_ca: if insecure { RootCaSource::Skip } else { RootCaSource::System },
			client_cert_hash: None,
			crl_sources: Vec::new(),
			verify_mode: if insecure { VerifyMode::Skip } else { VerifyMode::Full },
			alpn_protocols: alpn,
		}
	}

	#[test]
	fn fingerprint_eq_same_inputs() {
		let a = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: Some(sample_tls_fp(false, vec![b"h2".to_vec(), b"http/1.1".to_vec()])),
			dns: DnsConfig::System,
		};
		let b = a.clone();
		assert_eq!(a, b);
	}

	#[test]
	fn fingerprint_neq_different_version() {
		let a = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: Some(sample_tls_fp(false, vec![b"h2".to_vec(), b"http/1.1".to_vec()])),
			dns: DnsConfig::System,
		};
		let b = ClientFingerprint {
			version: UpstreamVersion::Http1,
			tls: Some(sample_tls_fp(false, vec![b"http/1.1".to_vec()])),
			dns: DnsConfig::System,
		};
		assert_ne!(a, b);
	}

	#[test]
	fn fingerprint_neq_secure_vs_insecure() {
		let a = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: Some(sample_tls_fp(false, vec![b"h2".to_vec()])),
			dns: DnsConfig::System,
		};
		let b = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: Some(sample_tls_fp(true, vec![b"h2".to_vec()])),
			dns: DnsConfig::System,
		};
		assert_ne!(a, b, "System and Skip must hash to distinct fingerprints");
	}

	#[test]
	fn fingerprint_eq_cleartext_same_version() {
		let a =
			ClientFingerprint { version: UpstreamVersion::Http1, tls: None, dns: DnsConfig::System };
		let b =
			ClientFingerprint { version: UpstreamVersion::Http1, tls: None, dns: DnsConfig::System };
		assert_eq!(a, b);
	}

	#[test]
	fn fingerprint_neq_cleartext_different_version() {
		let a =
			ClientFingerprint { version: UpstreamVersion::Http1, tls: None, dns: DnsConfig::System };
		let b =
			ClientFingerprint { version: UpstreamVersion::Http2, tls: None, dns: DnsConfig::System };
		assert_ne!(a, b);
	}

	#[test]
	fn fingerprint_neq_different_dns_nameservers() {
		let a = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: None,
			dns: DnsConfig::Custom(vec!["1.1.1.1:53".parse().unwrap()]),
		};
		let b = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: None,
			dns: DnsConfig::Custom(vec!["8.8.8.8:53".parse().unwrap()]),
		};
		assert_ne!(a, b, "different nameserver lists must produce distinct fingerprints");
	}

	#[test]
	fn fingerprint_neq_dns_order_swap() {
		let a = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: None,
			dns: DnsConfig::Custom(vec!["1.1.1.1:53".parse().unwrap(), "8.8.8.8:53".parse().unwrap()]),
		};
		let b = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: None,
			dns: DnsConfig::Custom(vec!["8.8.8.8:53".parse().unwrap(), "1.1.1.1:53".parse().unwrap()]),
		};
		assert_ne!(a, b, "primary/secondary order must be load-bearing");
	}

	fn make_dummy_client() -> ProxyClient {
		// Cheap construction: HttpsConnector with empty roots, never
		// drives a real handshake in the test (we only assert the
		// `Arc<Client>` identity, not behavior). Resolver matches the
		// production wiring so the connector type aligns with
		// `ProxyClient`.
		let cfg = rustls::ClientConfig::builder()
			.with_root_certificates(rustls::RootCertStore::empty())
			.with_no_client_auth();
		let resolver = HickoryDnsResolver::build(&DnsConfig::System).expect("system hickory resolver");
		let mut http = HttpConnector::new_with_resolver(resolver);
		http.enforce_http(false);
		let https = hyper_rustls::HttpsConnectorBuilder::new()
			.with_tls_config(cfg)
			.https_or_http()
			.enable_http1()
			.enable_http2()
			.wrap_connector(http);
		hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).build(https)
	}

	#[test]
	fn get_or_build_returns_same_arc_on_second_call() {
		clear_cache_for_test();
		crate::crypto::install_default_provider();
		let fp = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: Some(sample_tls_fp(true, vec![b"h2".to_vec(), b"http/1.1".to_vec()])),
			dns: DnsConfig::System,
		};
		let build_count = Arc::new(AtomicUsize::new(0));
		let bc = Arc::clone(&build_count);
		let first = get_or_build(fp.clone(), move || {
			bc.fetch_add(1, Ordering::SeqCst);
			make_dummy_client()
		});
		let bc = Arc::clone(&build_count);
		let second = get_or_build(fp, move || {
			bc.fetch_add(1, Ordering::SeqCst);
			make_dummy_client()
		});
		assert!(Arc::ptr_eq(&first, &second), "second lookup must return the cached Arc");
		assert_eq!(build_count.load(Ordering::SeqCst), 1, "build closure runs at most once");
	}

	#[test]
	fn get_or_build_builds_separately_for_different_fingerprints() {
		clear_cache_for_test();
		crate::crypto::install_default_provider();
		let fp_a = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: Some(sample_tls_fp(true, vec![b"h2".to_vec()])),
			dns: DnsConfig::System,
		};
		let fp_b =
			ClientFingerprint { version: UpstreamVersion::Http1, tls: None, dns: DnsConfig::System };
		let a = get_or_build(fp_a, make_dummy_client);
		let b = get_or_build(fp_b, make_dummy_client);
		assert!(!Arc::ptr_eq(&a, &b));
		assert!(cache_len() >= 2);
	}

	#[test]
	fn snapshot_decodes_https_entry_fields() {
		clear_cache_for_test();
		crate::crypto::install_default_provider();
		let fp = ClientFingerprint {
			version: UpstreamVersion::Http2,
			tls: Some(sample_tls_fp(false, vec![b"h2".to_vec()])),
			dns: DnsConfig::System,
		};
		let _ = get_or_build(fp, make_dummy_client);
		let entry = snapshot()
			.into_iter()
			.find(|s| s.version == "h2" && s.scheme == "https" && s.alpn == ["h2"])
			.expect("https h2 entry should be present");
		assert_eq!(entry.root_ca, "system");
		assert_eq!(entry.verify_mode, "full");
		assert_eq!(entry.dns, "system");
	}

	#[test]
	fn snapshot_decodes_cleartext_entry_fields() {
		clear_cache_for_test();
		crate::crypto::install_default_provider();
		let fp =
			ClientFingerprint { version: UpstreamVersion::Http1, tls: None, dns: DnsConfig::System };
		let _ = get_or_build(fp, make_dummy_client);
		let entry = snapshot()
			.into_iter()
			.find(|s| s.version == "h1" && s.scheme == "http")
			.expect("cleartext h1 entry should be present");
		assert_eq!(entry.root_ca, "none");
		assert_eq!(entry.verify_mode, "none");
		assert!(entry.alpn.is_empty());
		assert_eq!(entry.dns, "system");
	}

	#[test]
	fn fingerprint_id_is_stable_for_same_inputs() {
		let fp = ClientFingerprint {
			version: UpstreamVersion::Auto,
			tls: Some(sample_tls_fp(false, vec![b"h2".to_vec()])),
			dns: DnsConfig::System,
		};
		assert_eq!(fingerprint_id(&fp), fingerprint_id(&fp.clone()));
		assert_eq!(fingerprint_id(&fp).len(), 16);
	}

	#[test]
	fn fingerprint_id_differs_for_distinct_fingerprints() {
		let a =
			ClientFingerprint { version: UpstreamVersion::Http1, tls: None, dns: DnsConfig::System };
		let b =
			ClientFingerprint { version: UpstreamVersion::Http2, tls: None, dns: DnsConfig::System };
		assert_ne!(fingerprint_id(&a), fingerprint_id(&b));
	}

	#[test]
	fn drain_removes_matching_entry_only() {
		clear_cache_for_test();
		crate::crypto::install_default_provider();
		let fp_a =
			ClientFingerprint { version: UpstreamVersion::Http1, tls: None, dns: DnsConfig::System };
		let fp_b =
			ClientFingerprint { version: UpstreamVersion::Http2, tls: None, dns: DnsConfig::System };
		let _ = get_or_build(fp_a.clone(), make_dummy_client);
		let _ = get_or_build(fp_b.clone(), make_dummy_client);
		let id_a = fingerprint_id(&fp_a);
		let removed = drain_by_fingerprint_id(&id_a);
		assert_eq!(removed, 1);
		// `fp_a` is gone; `fp_b` still present.
		assert!(snapshot().iter().all(|s| s.fingerprint_id != id_a));
		assert!(snapshot().iter().any(|s| s.fingerprint_id == fingerprint_id(&fp_b)));
	}

	// In-flight Arc<Client> survives drain: the live reference returned
	// before drain stays usable, only future cache lookups see the
	// removal.
	#[test]
	fn drain_does_not_invalidate_existing_arc_clients() {
		clear_cache_for_test();
		crate::crypto::install_default_provider();
		let fp =
			ClientFingerprint { version: UpstreamVersion::Http1, tls: None, dns: DnsConfig::System };
		let live = get_or_build(fp.clone(), make_dummy_client);
		let id = fingerprint_id(&fp);
		assert_eq!(drain_by_fingerprint_id(&id), 1);
		// Cache miss now.
		assert!(snapshot().iter().all(|s| s.fingerprint_id != id));
		// Live `Arc<Client>` still alive — the drain only removes the
		// cache's strong reference.
		assert!(Arc::strong_count(&live) >= 1);
	}
}
