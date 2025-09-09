/* src/setup.rs */

use crate::config;
use anyhow::{Context, Result};
use fancy_log::{LogLevel, log};
use rcgen::generate_simple_self_signed;
use std::{fs, path::Path};

const DEFAULT_MAIN_CONFIG: &str = r#"
# Vane configuration file
[domains]
"example.com" = { file = "example.com.toml", tls = { cert = "~/vane/certs/example.com.pem", key = "~/vane/certs/example.com.key" } }
"#;

const DEFAULT_DOMAIN_CONFIG: &str = r#"
# Routing rules for example.com
[[routes]]
path = "/"
targets = ["http://127.0.0.1:5174"]
"#;

/// Handles the first-run scenario by creating directories, certs, and configs.
pub async fn handle_first_run() -> Result<()> {
    log(
        LogLevel::Warn,
        "No domains configured. Performing first-time setup.",
    );
    log(
        LogLevel::Info,
        "For guidance, visit: https://github.com/canmi21/vane",
    );

    let (config_path, config_dir) = config::get_config_paths()?;
    let certs_dir = config_dir.join("certs");
    fs::create_dir_all(&certs_dir).context("Failed to create certs directory")?;

    let cert_path = certs_dir.join("example.com.pem");
    let key_path = certs_dir.join("example.com.key");

    if !cert_path.exists() || !key_path.exists() {
        log(
            LogLevel::Info,
            "Generating self-signed certificate for example.com...",
        );
        generate_self_signed_cert("example.com", &cert_path, &key_path)?;
    }

    if !config_path.exists() {
        fs::write(&config_path, DEFAULT_MAIN_CONFIG)?;
        log(
            LogLevel::Info,
            &format!("Created example config: {:?}", config_path),
        );
    }

    let domain_config_path = config_dir.join("example.com.toml");
    if !domain_config_path.exists() {
        fs::write(&domain_config_path, DEFAULT_DOMAIN_CONFIG)?;
        log(
            LogLevel::Info,
            &format!("Created domain config: {:?}", domain_config_path),
        );
    }

    log(
        LogLevel::Info,
        "First-time setup complete. Please start Vane again.",
    );
    Ok(())
}

/// Generates a self-signed certificate and private key using the rcgen crate.
fn generate_self_signed_cert(hostname: &str, cert_path: &Path, key_path: &Path) -> Result<()> {
    let cert = generate_simple_self_signed(vec![hostname.to_string()])?;
    // Use the modern rcgen API for PEM serialization
    fs::write(cert_path, cert.cert.pem())?;
    fs::write(key_path, cert.signing_key.serialize_pem())?;
    Ok(())
}
