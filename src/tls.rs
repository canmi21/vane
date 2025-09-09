/* src/tls.rs */

use crate::config::AppConfig;
use anyhow::{Context, Result, anyhow};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

/// A resolver that provides a certificate and key on-the-fly, based on the
/// server name indication (SNI) from the client.
#[derive(Debug)] // Corrected: Added derive(Debug) to satisfy trait bounds.
pub struct PerDomainCertResolver {
    app_config: Arc<AppConfig>,
}

impl PerDomainCertResolver {
    pub fn new(app_config: Arc<AppConfig>) -> Self {
        Self { app_config }
    }
}

impl ResolvesServerCert for PerDomainCertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<sign::CertifiedKey>> {
        let server_name = match client_hello.server_name() {
            Some(name) => name,
            None => {
                fancy_log::log(
                    fancy_log::LogLevel::Warn,
                    "TLS client did not provide SNI. Cannot serve certificate.",
                );
                return None;
            }
        };

        // Find the configuration for the requested domain.
        let domain_config = self.app_config.domains.get(server_name)?;

        // Check if HTTPS is enabled for this domain and if TLS config exists.
        if !domain_config.https {
            return None;
        }
        let tls_config = domain_config.tls.as_ref()?;

        // Attempt to build the certified key for the requested domain.
        match build_certified_key(tls_config) {
            Ok(key) => Some(Arc::new(key)),
            Err(e) => {
                fancy_log::log(
                    fancy_log::LogLevel::Error,
                    &format!("Failed to build TLS cert for {}: {}", server_name, e),
                );
                None
            }
        }
    }
}

/// Loads certificates, key, and creates a `CertifiedKey`.
fn build_certified_key(config: &crate::models::TlsConfig) -> Result<sign::CertifiedKey> {
    let cert_path = shellexpand::tilde(&config.cert).into_owned();
    let key_path = shellexpand::tilde(&config.key).into_owned();

    let certs = load_certs(&cert_path)?;
    let key = load_key(&key_path)?;

    // Corrected: Use the full path to the function from the default crypto provider.
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|e| anyhow!("Failed to create signing key: {}", e))?;

    Ok(sign::CertifiedKey::new(certs, signing_key))
}

/// Loads and parses a PEM-encoded certificate file.
fn load_certs<'a>(path: &str) -> Result<Vec<CertificateDer<'a>>> {
    let file = File::open(path).with_context(|| format!("Failed to open cert file: {}", path))?;
    let mut reader = BufReader::new(file);
    // Corrected: Use context() to convert std::io::Error into anyhow::Error.
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to parse certificates from PEM file")
}

/// Loads and parses a PEM-encoded private key file.
fn load_key<'a>(path: &str) -> Result<PrivateKeyDer<'a>> {
    let file = File::open(path).with_context(|| format!("Failed to open key file: {}", path))?;
    let mut reader = BufReader::new(file);
    // Corrected: Use context() to convert std::io::Error into anyhow::Error.
    rustls_pemfile::private_key(&mut reader)
        .context("Failed to find private key in PEM file")?
        .context("Failed to parse private key PEM")
}
