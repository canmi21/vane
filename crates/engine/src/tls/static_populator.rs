//! `StaticCertPopulator`: PEM-on-disk populator. Loads cert / key
//! files at link time; OCSP staples come from one of three sources
//! per per-rule config (none / `ocsp_path` / `ocsp_fetch`).
//!
//! Operators rotate static certs by editing the rule-set and
//! triggering a daemon-side reload, which rebuilds the populator
//! from scratch — there is no in-place cert refresh. OCSP staples
//! are the one piece of state the static populator does refresh
//! periodically: file-backed `ocsp_path` re-reads on every
//! `refresh()` (no mtime check; cheap), `ocsp_fetch` re-fetches
//! when the cached `nextUpdate` falls inside
//! `OCSP_REFRESH_BEFORE` of expiry.
//!
//! Spec: `spec/crates/engine-tls.md` § _Cert populators_ (Built-in implementations)
//! and § _OCSP stapling_ (transport policy + refresh cadence).

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use parking_lot::Mutex;
use vane_core::rule::{ListenerTlsSpec, TlsConfig};
use x509_parser::prelude::FromDer;

use crate::tls::ocsp::{self, FETCH_TIMEOUT, OcspError};
use crate::tls::populator::{CertPopulator, PopulatorError};
use crate::tls::{CertEntry, CertStore};

/// `nextUpdate` proximity that triggers a fresh fetch on
/// `ocsp_fetch: true` configs. Mirrors
/// [`crate::acme::scheduler::OCSP_REFRESH_BEFORE`] so static and
/// managed populators share one OCSP cadence.
const OCSP_REFRESH_BEFORE: Duration = Duration::from_hours(24);

/// Cached fetched-OCSP staple per cert (i.e. per `TlsConfig` slot).
/// Lives in a `Mutex` so the otherwise-immutable populator can
/// memoise a successful fetch across `refresh()` calls without
/// re-doing the network round-trip every 5-minute tick.
#[derive(Debug, Default, Clone)]
struct CachedOcsp {
	staple: Option<Vec<u8>>,
	next_update: Option<SystemTime>,
}

#[derive(Debug)]
pub struct StaticCertPopulator {
	default: Option<TlsConfig>,
	by_sni: Vec<(String, TlsConfig)>,
	/// `ocsp_fetch: true` cache, keyed by `(slot_kind, sni)` —
	/// `slot_kind = "default"` or the SNI itself. `None` means
	/// "no fetch attempted yet"; see [`Self::cache_for`].
	fetch_cache: Mutex<HashMap<String, CachedOcsp>>,
}

impl StaticCertPopulator {
	/// Snapshot the spec's PEM paths into a populator. Returns an
	/// error only on shape problems (empty spec); PEM I/O happens
	/// lazily in [`Self::initial_store_sync`] so the caller's error
	/// site is uniform.
	///
	/// # Errors
	/// `PopulatorError::Source` if the spec carries neither a default
	/// cert nor any SNI-keyed cert.
	pub fn from_spec(spec: &ListenerTlsSpec) -> Result<Self, PopulatorError> {
		if spec.is_empty() {
			return Err(PopulatorError::source("listener TLS spec is empty (no default + no sni certs)"));
		}
		let by_sni = spec.sni_certs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
		Ok(Self { default: spec.default.clone(), by_sni, fetch_cache: Mutex::new(HashMap::new()) })
	}

	/// Synchronous twin of [`Self::initial_store`]. PEM reads + the
	/// file-based OCSP path cost a few ms on cold disk, so we stay
	/// sync — `FlowGraph::link` is itself sync. The async wrapper
	/// exists only so the trait shape can host populators (ACME /
	/// managed) that genuinely need `await`. **`ocsp_fetch: true`
	/// configs do not perform their network fetch here**; they
	/// surface without a staple at link time and the first
	/// [`Self::refresh`] tick populates the cache.
	///
	/// # Errors
	/// `PopulatorError::Source` for any I/O, PEM-parse, signing-key,
	/// or x509 `notAfter` parse failure.
	pub fn initial_store_sync(&self) -> Result<CertStore, PopulatorError> {
		let cache = self.fetch_cache.lock().clone();
		let default = self
			.default
			.as_ref()
			.map(|tls| load_entry(tls, cache.get("default").cloned().unwrap_or_default()).map(Arc::new))
			.transpose()?;
		let mut by_sni: HashMap<String, Arc<CertEntry>> = HashMap::with_capacity(self.by_sni.len());
		for (sni, tls) in &self.by_sni {
			// `lower` already lowercases the key; assert it as a
			// belt-and-suspenders for any post-lower meta tampering.
			debug_assert_eq!(sni, &sni.to_ascii_lowercase());
			let cached = cache.get(sni).cloned().unwrap_or_default();
			by_sni.insert(sni.clone(), Arc::new(load_entry(tls, cached)?));
		}
		Ok(CertStore { by_sni, default })
	}

	/// Walk every `ocsp_fetch: true` cert and run the OCSP fetch
	/// pipeline when the cached staple is absent or near expiry.
	/// Updates [`Self::fetch_cache`] in place; the caller decides
	/// whether to rebuild the `CertStore` based on whether anything
	/// changed.
	async fn refresh_fetch_cache(&self) -> bool {
		let mut any_change = false;
		// Default slot.
		if let Some(tls) = &self.default
			&& tls.ocsp_fetch
		{
			let cached = self.fetch_cache.lock().get("default").cloned().unwrap_or_default();
			if let Some(updated) = fetch_one(tls, &cached).await {
				self.fetch_cache.lock().insert("default".into(), updated);
				any_change = true;
			}
		}
		// SNI slots.
		for (sni, tls) in &self.by_sni {
			if !tls.ocsp_fetch {
				continue;
			}
			let cached = self.fetch_cache.lock().get(sni).cloned().unwrap_or_default();
			if let Some(updated) = fetch_one(tls, &cached).await {
				self.fetch_cache.lock().insert(sni.clone(), updated);
				any_change = true;
			}
		}
		any_change
	}
}

#[async_trait]
impl CertPopulator for StaticCertPopulator {
	async fn initial_store(&self) -> Result<CertStore, PopulatorError> {
		self.initial_store_sync()
	}

	/// Refresh the cert store. Cert PEMs themselves never change at
	/// runtime (operators rotate via config reload), so the only
	/// thing that can flip is the OCSP staple — re-read
	/// `ocsp_path` files (cheap, idempotent), and re-fetch
	/// `ocsp_fetch` URLs when their cached `nextUpdate` is within
	/// `OCSP_REFRESH_BEFORE`. Returns `Some(new_store)` only when
	/// at least one staple changed; `None` is the steady state.
	async fn refresh(&self, current: &CertStore) -> Result<Option<CertStore>, PopulatorError> {
		// Run the network refresh first; it updates the in-memory
		// cache. `initial_store_sync` then assembles a fresh store
		// observing both the (possibly-rewritten) `ocsp_path` files
		// and the (possibly-renewed) fetch cache.
		let _refetched = self.refresh_fetch_cache().await;
		let candidate = self.initial_store_sync()?;
		if cert_stores_same_staples(current, &candidate) { Ok(None) } else { Ok(Some(candidate)) }
	}
}

/// Compare two `CertStore`s on the dimensions a static populator
/// can change — namely OCSP staple bytes per slot. Cert chains
/// don't change without a config reload; comparing them would just
/// be wasted DER decoding.
fn cert_stores_same_staples(a: &CertStore, b: &CertStore) -> bool {
	if a.by_sni.len() != b.by_sni.len() {
		return false;
	}
	for (sni, ae) in &a.by_sni {
		let Some(be) = b.by_sni.get(sni) else { return false };
		if ae.key.ocsp != be.key.ocsp {
			return false;
		}
	}
	match (&a.default, &b.default) {
		(None, None) => true,
		(Some(ad), Some(bd)) => ad.key.ocsp == bd.key.ocsp,
		_ => false,
	}
}

/// Run the OCSP fetch for one `TlsConfig` slot and decide whether
/// the cached entry needs to be replaced. Returns `Some(new)` when
/// the cache should be updated; `None` when the existing cache is
/// fine (or the fetch failed and we're keeping the current state).
async fn fetch_one(tls: &TlsConfig, cached: &CachedOcsp) -> Option<CachedOcsp> {
	debug_assert!(tls.ocsp_fetch, "fetch_one called on non-fetch slot");
	let now = SystemTime::now();
	let needs_fetch = match (&cached.staple, cached.next_update) {
		(None, _) => true,
		(Some(_), Some(nu)) => {
			nu.checked_sub(OCSP_REFRESH_BEFORE).is_none_or(|deadline| now >= deadline)
		}
		(Some(_), None) => false,
	};
	if !needs_fetch {
		return None;
	}
	let (cert_path, _key_path) = tls.static_paths()?;
	let cert_bytes = match fs::read(cert_path) {
		Ok(b) => b,
		Err(e) => {
			tracing::warn!(
				target: "vane::tls::ocsp",
				path = %cert_path.display(),
				error = %e,
				"OCSP fetch: cert read failed",
			);
			return None;
		}
	};
	// Need leaf + first intermediate as the issuer. Drop the iter
	// binding before the `await` below so its non-Send capture
	// doesn't leak into the future's auto-trait inference.
	let (leaf, issuer) = {
		let mut slice: &[u8] = cert_bytes.as_slice();
		let mut iter = rustls_pemfile::certs(&mut slice);
		let Some(Ok(leaf)) = iter.next() else { return None };
		let Some(Ok(issuer)) = iter.next() else { return None };
		(leaf, issuer)
	};
	match ocsp::fetch_ocsp_for_cert(leaf.as_ref(), issuer.as_ref(), FETCH_TIMEOUT).await {
		Ok(staple) => {
			Some(CachedOcsp { staple: Some(staple.staple), next_update: Some(staple.next_update) })
		}
		Err(OcspError::HttpsNotSupported(url)) => {
			tracing::warn!(
				target: "vane::tls::ocsp",
				%url,
				"OCSP responder URL is HTTPS — vane fetches OCSP only over HTTP; staple deferred",
			);
			None
		}
		Err(OcspError::NoAia | OcspError::NoOcspUrl) => {
			tracing::debug!(
				target: "vane::tls::ocsp",
				"cert has no AIA OCSP URL; skipping fetch",
			);
			None
		}
		Err(e) => {
			tracing::warn!(
				target: "vane::tls::ocsp",
				error = %e,
				"OCSP fetch failed; will retry on next refresh tick",
			);
			None
		}
	}
}

fn load_entry(tls: &TlsConfig, fetched: CachedOcsp) -> Result<CertEntry, PopulatorError> {
	// `StaticCertPopulator` only ever sees configs the lower pass routed
	// into `default` / `sni_certs`, both of which carry static disk
	// paths by invariant. Surface a typed error rather than panicking
	// if some upstream caller hands us a managed config by mistake.
	let (cert_path, key_path) = tls.static_paths().ok_or_else(|| {
		PopulatorError::source("StaticCertPopulator received a managed TlsConfig — engine routing bug")
	})?;
	let cert_bytes = fs::read(cert_path)
		.map_err(|e| PopulatorError::source(format!("read cert_file {}: {e}", cert_path.display())))?;
	let key_bytes = fs::read(key_path)
		.map_err(|e| PopulatorError::source(format!("read key_file {}: {e}", key_path.display())))?;

	let cert_chain: Vec<rustls::pki_types::CertificateDer<'static>> =
		rustls_pemfile::certs(&mut cert_bytes.as_slice()).collect::<Result<_, _>>().map_err(|e| {
			PopulatorError::source(format!("parse cert_file {}: {e}", cert_path.display()))
		})?;
	if cert_chain.is_empty() {
		return Err(PopulatorError::source(format!(
			"cert_file {} contained no certificates",
			cert_path.display(),
		)));
	}

	let private_key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
		.map_err(|e| PopulatorError::source(format!("parse key_file {}: {e}", key_path.display())))?
		.ok_or_else(|| {
			PopulatorError::source(format!("key_file {} contained no private key", key_path.display()))
		})?;

	let provider = rustls::crypto::CryptoProvider::get_default()
		.ok_or_else(|| PopulatorError::source("rustls crypto provider not installed"))?;
	let signing_key = provider
		.key_provider
		.load_private_key(private_key)
		.map_err(|e| PopulatorError::source(format!("load_private_key {}: {e}", key_path.display())))?;

	let not_after = parse_not_after(cert_chain[0].as_ref())
		.map_err(|e| PopulatorError::source(format!("parse notAfter {}: {e}", cert_path.display())))?;

	// Resolve the OCSP staple per per-rule config. Three sources:
	// (a) `ocsp_path`: read-and-parse the operator-supplied DER on
	//     every load. Cheap; failure logs WARN and ships without
	//     a staple (cert is still usable).
	// (b) `ocsp_fetch`: read from the in-memory `fetched` cache —
	//     `Self::refresh_fetch_cache` populates it asynchronously
	//     out of band.
	// (c) Neither: no staple.
	let (ocsp_response, ocsp_next_update) = match (&tls.ocsp_path, tls.ocsp_fetch) {
		(Some(path), false) => match fs::read(path) {
			Ok(bytes) => match ocsp::parse_ocsp_response(&bytes) {
				Ok(staple) => (Some(staple.staple), Some(staple.next_update)),
				Err(e) => {
					tracing::warn!(
						target: "vane::tls::ocsp",
						path = %path.display(),
						error = %e,
						"ocsp_path parse failed; cert ships without staple",
					);
					(None, None)
				}
			},
			Err(e) => {
				tracing::warn!(
					target: "vane::tls::ocsp",
					path = %path.display(),
					error = %e,
					"ocsp_path read failed; cert ships without staple",
				);
				(None, None)
			}
		},
		(None, true) => (fetched.staple, fetched.next_update),
		(None, false) => (None, None),
		// Compile-time validation in `TlsConfig::validate` rejects
		// `Some(_) + true`; reaching here would be a routing bug.
		(Some(_), true) => unreachable!("ocsp_path + ocsp_fetch combo blocked at compile"),
	};

	let mut certified = rustls::sign::CertifiedKey::new(cert_chain, signing_key);
	certified.ocsp = ocsp_response;
	Ok(CertEntry {
		key: Arc::new(certified),
		not_after,
		ocsp_next_update: ocsp_next_update.and_then(system_time_to_instant),
	})
}

/// Wall-clock → monotonic conversion. Mirrors the helper in
/// [`crate::acme::populator`] so the listener-side refresh loop can
/// compare against `Instant::now()` regardless of which populator
/// produced the entry.
fn system_time_to_instant(target: SystemTime) -> Option<std::time::Instant> {
	let now_sys = SystemTime::now();
	let now_inst = std::time::Instant::now();
	target.duration_since(now_sys).ok().map(|delta| now_inst + delta)
}

fn parse_not_after(der: &[u8]) -> Result<SystemTime, String> {
	let (_, cert) =
		x509_parser::prelude::X509Certificate::from_der(der).map_err(|e| format!("{e}"))?;
	let secs = cert.validity().not_after.timestamp();
	if secs < 0 {
		return Err(format!("notAfter has negative epoch {secs}"));
	}
	#[expect(
		clippy::cast_sign_loss,
		reason = "non-negativity verified above; secs is `i64` from x509-parser"
	)]
	let secs = secs as u64;
	Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;
	use std::io::Write as _;
	use std::path::PathBuf;
	use std::time::Duration;

	use tempfile::NamedTempFile;

	use super::*;

	fn install_crypto() {
		crate::crypto::install_default_provider();
	}

	fn write_pem(contents: &str) -> NamedTempFile {
		let mut f = NamedTempFile::new().expect("tmpfile");
		f.write_all(contents.as_bytes()).expect("write pem");
		f
	}

	fn rcgen_self_signed() -> (String, String) {
		let issued =
			rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("self-signed cert");
		(issued.cert.pem(), issued.signing_key.serialize_pem())
	}

	fn default_only(cert_path: PathBuf, key_path: PathBuf) -> ListenerTlsSpec {
		ListenerTlsSpec {
			default: Some(TlsConfig {
				sni: None,
				cert_file: Some(cert_path),
				key_file: Some(key_path),
				managed: None,
				enable_zero_rtt: false,
				client_auth: None,
				ocsp_path: None,
				ocsp_fetch: false,
			}),
			sni_certs: BTreeMap::new(),
			managed_snis: BTreeMap::new(),
			client_auth: vane_core::rule::ClientAuthSpec::None,
			enable_zero_rtt: false,
		}
	}

	#[test]
	fn from_spec_loads_default_pem_and_parses_not_after() {
		install_crypto();
		let (cert_pem, key_pem) = rcgen_self_signed();
		let cert = write_pem(&cert_pem);
		let key = write_pem(&key_pem);
		let spec = default_only(cert.path().to_path_buf(), key.path().to_path_buf());
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		let store = pop.initial_store_sync().expect("initial_store_sync");
		let entry = store.default.expect("default present");
		assert!(store.by_sni.is_empty());
		// rcgen issues self-signed certs valid for the next year by default.
		let now = SystemTime::now();
		let lower = now + Duration::from_hours(360 * 24);
		assert!(
			entry.not_after >= lower,
			"not_after {:?} should be at least 360 days from now",
			entry.not_after,
		);
		assert!(entry.ocsp_next_update.is_none());
	}

	#[test]
	fn from_spec_loads_sni_keyed_pem() {
		install_crypto();
		let (cert_pem, key_pem) = rcgen_self_signed();
		let cert = write_pem(&cert_pem);
		let key = write_pem(&key_pem);
		let mut sni_certs = BTreeMap::new();
		sni_certs.insert(
			"api.example.com".to_owned(),
			TlsConfig {
				sni: Some("api.example.com".to_owned()),
				cert_file: Some(cert.path().to_path_buf()),
				key_file: Some(key.path().to_path_buf()),
				managed: None,
				client_auth: None,
				enable_zero_rtt: false,
				ocsp_path: None,
				ocsp_fetch: false,
			},
		);
		let spec = ListenerTlsSpec {
			default: None,
			sni_certs,
			managed_snis: BTreeMap::new(),
			client_auth: vane_core::rule::ClientAuthSpec::None,
			enable_zero_rtt: false,
		};
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		let store = pop.initial_store_sync().expect("initial_store_sync");
		assert!(store.default.is_none());
		assert!(store.by_sni.contains_key("api.example.com"));
	}

	#[test]
	fn from_spec_rejects_empty() {
		let spec = ListenerTlsSpec {
			default: None,
			sni_certs: BTreeMap::new(),
			managed_snis: BTreeMap::new(),
			client_auth: vane_core::rule::ClientAuthSpec::None,
			enable_zero_rtt: false,
		};
		let err = StaticCertPopulator::from_spec(&spec).expect_err("empty spec rejected");
		let msg = err.to_string();
		assert!(msg.contains("empty"), "{msg}");
	}

	#[test]
	fn missing_cert_file_errors() {
		install_crypto();
		let (_, key_pem) = rcgen_self_signed();
		let key = write_pem(&key_pem);
		let spec = default_only(PathBuf::from("/nonexistent/cert.pem"), key.path().to_path_buf());
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		let err = pop.initial_store_sync().expect_err("missing cert errors");
		assert!(err.to_string().contains("read cert_file"), "{err}");
	}

	#[test]
	fn garbage_cert_pem_errors() {
		install_crypto();
		let (_, key_pem) = rcgen_self_signed();
		let cert = write_pem("this is not a PEM cert\n");
		let key = write_pem(&key_pem);
		let spec = default_only(cert.path().to_path_buf(), key.path().to_path_buf());
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		let err = pop.initial_store_sync().expect_err("garbage cert errors");
		let msg = err.to_string();
		assert!(msg.contains("contained no certificates") || msg.contains("parse cert_file"), "{msg}");
	}

	#[test]
	fn key_file_without_private_key_errors() {
		install_crypto();
		let (cert_pem, _) = rcgen_self_signed();
		let cert = write_pem(&cert_pem);
		let key = write_pem("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n");
		let spec = default_only(cert.path().to_path_buf(), key.path().to_path_buf());
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		let err = pop.initial_store_sync().expect_err("missing private key errors");
		let msg = err.to_string();
		assert!(msg.contains("no private key") || msg.contains("parse key_file"), "{msg}");
	}

	#[tokio::test]
	async fn refresh_returns_none_for_static() {
		install_crypto();
		let (cert_pem, key_pem) = rcgen_self_signed();
		let cert = write_pem(&cert_pem);
		let key = write_pem(&key_pem);
		let spec = default_only(cert.path().to_path_buf(), key.path().to_path_buf());
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		let store = pop.initial_store().await.expect("initial_store");
		assert!(pop.refresh(&store).await.expect("refresh").is_none());
	}

	#[test]
	fn validate_rejects_both_ocsp_path_and_ocsp_fetch() {
		install_crypto();
		let (_, _) = rcgen_self_signed();
		let tls = TlsConfig {
			sni: Some("api.example".into()),
			cert_file: Some(PathBuf::from("/tmp/c.pem")),
			key_file: Some(PathBuf::from("/tmp/k.pem")),
			managed: None,
			enable_zero_rtt: false,
			client_auth: None,
			ocsp_path: Some(PathBuf::from("/tmp/ocsp.der")),
			ocsp_fetch: true,
		};
		let err = tls.validate().expect_err("both ocsp sources rejected");
		let msg = err.to_string();
		assert!(msg.contains("ocsp_path") && msg.contains("ocsp_fetch"), "{msg}");
	}

	#[test]
	fn from_spec_loads_ocsp_path_when_present() {
		install_crypto();
		let (cert_pem, key_pem) = rcgen_self_signed();
		let cert = write_pem(&cert_pem);
		let key = write_pem(&key_pem);

		// Pre-fetched OCSP DER on disk. We use a `try_later` response
		// here as a deterministic fixture — `parse_ocsp_response`
		// rejects non-successful statuses, so the populator's WARN
		// branch fires (no staple) but the cert still loads.
		// `OcspResponse::try_later()` is the simplest forge-free
		// fixture available without spinning up a signer.
		let try_later = x509_ocsp::OcspResponse::try_later();
		let bytes = der::Encode::to_der(&try_later).expect("encode");
		let ocsp_file = NamedTempFile::new().expect("ocsp tmp");
		std::fs::write(ocsp_file.path(), &bytes).expect("write");

		let spec = ListenerTlsSpec {
			default: Some(TlsConfig {
				sni: None,
				cert_file: Some(cert.path().to_path_buf()),
				key_file: Some(key.path().to_path_buf()),
				managed: None,
				enable_zero_rtt: false,
				client_auth: None,
				ocsp_path: Some(ocsp_file.path().to_path_buf()),
				ocsp_fetch: false,
			}),
			sni_certs: BTreeMap::new(),
			managed_snis: BTreeMap::new(),
			client_auth: vane_core::rule::ClientAuthSpec::None,
			enable_zero_rtt: false,
		};
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		let store = pop.initial_store_sync().expect("initial_store_sync");
		let entry = store.default.expect("default present");
		// `try_later` parses as ResponderError → populator drops to
		// "no staple" branch. The cert is still loaded.
		assert!(entry.key.ocsp.is_none(), "non-successful response → no staple");
	}

	#[test]
	fn ocsp_path_with_garbage_bytes_logs_warn_and_proceeds() {
		install_crypto();
		let (cert_pem, key_pem) = rcgen_self_signed();
		let cert = write_pem(&cert_pem);
		let key = write_pem(&key_pem);
		let bad_ocsp = write_pem("definitely not OCSP DER");
		let spec = ListenerTlsSpec {
			default: Some(TlsConfig {
				sni: None,
				cert_file: Some(cert.path().to_path_buf()),
				key_file: Some(key.path().to_path_buf()),
				managed: None,
				enable_zero_rtt: false,
				client_auth: None,
				ocsp_path: Some(bad_ocsp.path().to_path_buf()),
				ocsp_fetch: false,
			}),
			sni_certs: BTreeMap::new(),
			managed_snis: BTreeMap::new(),
			client_auth: vane_core::rule::ClientAuthSpec::None,
			enable_zero_rtt: false,
		};
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		let store = pop.initial_store_sync().expect("initial_store_sync");
		let entry = store.default.expect("default present");
		assert!(entry.key.ocsp.is_none(), "garbage OCSP → no staple, cert loaded");
	}

	#[tokio::test]
	async fn ocsp_fetch_true_starts_with_no_staple_at_link_time() {
		install_crypto();
		let (cert_pem, key_pem) = rcgen_self_signed();
		let cert = write_pem(&cert_pem);
		let key = write_pem(&key_pem);
		let spec = ListenerTlsSpec {
			default: Some(TlsConfig {
				sni: None,
				cert_file: Some(cert.path().to_path_buf()),
				key_file: Some(key.path().to_path_buf()),
				managed: None,
				enable_zero_rtt: false,
				client_auth: None,
				ocsp_path: None,
				ocsp_fetch: true,
			}),
			sni_certs: BTreeMap::new(),
			managed_snis: BTreeMap::new(),
			client_auth: vane_core::rule::ClientAuthSpec::None,
			enable_zero_rtt: false,
		};
		let pop = StaticCertPopulator::from_spec(&spec).expect("from_spec");
		// Link-time `initial_store` does NOT do network IO; the
		// staple is None until the first `refresh()` runs.
		let store = pop.initial_store().await.expect("initial_store");
		let entry = store.default.expect("default present");
		assert!(entry.key.ocsp.is_none(), "ocsp_fetch is async — no staple at link time");
	}
}
