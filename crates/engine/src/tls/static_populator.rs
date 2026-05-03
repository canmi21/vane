//! `StaticCertPopulator`: PEM-on-disk populator. Stateless. Loads
//! cert / key files at link time and never re-reads them — operators
//! rotate static certs by editing the rule-set and triggering a
//! daemon-side reload, which rebuilds the populator from scratch.
//!
//! Spec: `08-tls.md` § _Cert populators_ (Built-in implementations).

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use vane_core::rule::{ListenerTlsSpec, TlsConfig};
use x509_parser::prelude::FromDer;

use crate::tls::populator::{CertPopulator, PopulatorError};
use crate::tls::{CertEntry, CertStore};

#[derive(Debug)]
pub struct StaticCertPopulator {
	default: Option<TlsConfig>,
	by_sni: Vec<(String, TlsConfig)>,
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
		Ok(Self { default: spec.default.clone(), by_sni })
	}

	/// Synchronous twin of [`Self::initial_store`]. PEM reads cost a
	/// few ms on cold disk, so we stay sync — `FlowGraph::link` is
	/// itself sync. The async wrapper exists only so the trait shape
	/// can host populators (ACME / managed) that genuinely need
	/// `await`.
	///
	/// # Errors
	/// `PopulatorError::Source` for any I/O, PEM-parse, signing-key,
	/// or x509 `notAfter` parse failure.
	pub fn initial_store_sync(&self) -> Result<CertStore, PopulatorError> {
		let default = self.default.as_ref().map(|tls| load_entry(tls).map(Arc::new)).transpose()?;
		let mut by_sni: HashMap<String, Arc<CertEntry>> = HashMap::with_capacity(self.by_sni.len());
		for (sni, tls) in &self.by_sni {
			// `lower` already lowercases the key; assert it as a
			// belt-and-suspenders for any post-lower meta tampering.
			debug_assert_eq!(sni, &sni.to_ascii_lowercase());
			by_sni.insert(sni.clone(), Arc::new(load_entry(tls)?));
		}
		Ok(CertStore { by_sni, default })
	}
}

#[async_trait]
impl CertPopulator for StaticCertPopulator {
	async fn initial_store(&self) -> Result<CertStore, PopulatorError> {
		self.initial_store_sync()
	}

	/// Static populators never report staleness — operators rotate
	/// disk PEMs through a config reload, which rebuilds the
	/// populator from scratch.
	async fn refresh(&self, _current: &CertStore) -> Result<Option<CertStore>, PopulatorError> {
		Ok(None)
	}
}

fn load_entry(tls: &TlsConfig) -> Result<CertEntry, PopulatorError> {
	let cert_bytes = fs::read(&tls.cert_file).map_err(|e| {
		PopulatorError::source(format!("read cert_file {}: {e}", tls.cert_file.display()))
	})?;
	let key_bytes = fs::read(&tls.key_file).map_err(|e| {
		PopulatorError::source(format!("read key_file {}: {e}", tls.key_file.display()))
	})?;

	let cert_chain: Vec<rustls::pki_types::CertificateDer<'static>> =
		rustls_pemfile::certs(&mut cert_bytes.as_slice()).collect::<Result<_, _>>().map_err(|e| {
			PopulatorError::source(format!("parse cert_file {}: {e}", tls.cert_file.display()))
		})?;
	if cert_chain.is_empty() {
		return Err(PopulatorError::source(format!(
			"cert_file {} contained no certificates",
			tls.cert_file.display(),
		)));
	}

	let private_key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
		.map_err(|e| PopulatorError::source(format!("parse key_file {}: {e}", tls.key_file.display())))?
		.ok_or_else(|| {
			PopulatorError::source(format!(
				"key_file {} contained no private key",
				tls.key_file.display(),
			))
		})?;

	let provider = rustls::crypto::CryptoProvider::get_default()
		.ok_or_else(|| PopulatorError::source("rustls crypto provider not installed"))?;
	let signing_key = provider.key_provider.load_private_key(private_key).map_err(|e| {
		PopulatorError::source(format!("load_private_key {}: {e}", tls.key_file.display()))
	})?;

	let not_after = parse_not_after(cert_chain[0].as_ref()).map_err(|e| {
		PopulatorError::source(format!("parse notAfter {}: {e}", tls.cert_file.display()))
	})?;

	Ok(CertEntry {
		key: Arc::new(rustls::sign::CertifiedKey::new(cert_chain, signing_key)),
		not_after,
		ocsp_next_update: None,
	})
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
				cert_file: cert_path,
				key_file: key_path,
				enable_zero_rtt: false,
				client_auth: None,
			}),
			sni_certs: BTreeMap::new(),
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
				cert_file: cert.path().to_path_buf(),
				key_file: key.path().to_path_buf(),
				client_auth: None,
				enable_zero_rtt: false,
			},
		);
		let spec = ListenerTlsSpec {
			default: None,
			sni_certs,
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
}
