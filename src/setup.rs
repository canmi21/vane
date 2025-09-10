/* src/setup.rs */

use crate::config;
use anyhow::{Context, Result};
use fancy_log::{LogLevel, log};
use include_dir::{Dir, include_dir};
use rcgen::generate_simple_self_signed;
use std::{fs, path::Path};

// Embeds the `./status` directory into the binary at compile time.
// `$CARGO_MANIFEST_DIR` resolves to the project's root directory (where Cargo.toml is).
static STATIC_STATUS_PAGES: Dir = include_dir!("$CARGO_MANIFEST_DIR/status");

const DEFAULT_MAIN_CONFIG: &str = r#"
# Vane main configuration file
# This file maps hostnames to their specific configuration files.
[domains]
"example.com" = "example.com.toml"
"#;

const DEFAULT_DOMAIN_CONFIG: &str = r#"
# Vane domain configuration for example.com

# Enable HTTPS for this domain. If true, a [tls] section must be provided.
https = true

# Enable HTTP/3 support for this domain.
# This requires the server's UDP port 443 (or the configured https_port) to be accessible.
http3 = false

# HSTS (HTTP Strict Transport Security) adds a header to HTTPS responses
# telling browsers to only connect via HTTPS in the future.
hsts = false

# Configure behavior for plain HTTP requests.
# "upgrade": Redirect HTTP to HTTPS (301 Moved Permanently).
# "reject": Reject the request (426 Upgrade Required).
# "allow": Serve content over HTTP (not recommended if HTTPS is enabled).
http_options = "upgrade"

# TLS configuration is required if https = true.
[tls]
cert = "~/vane/certs/example.com.pem"
key = "~/vane/certs/example.com.key"

# Routing rules for this domain.
[[routes]]
# The path prefix to match. "/" matches everything.
path = "/"
# The backend server(s) to proxy requests to.
targets = ["http://127.0.0.1:33433"]
"#;

/// Checks if the status pages directory exists. If not, it creates it and
/// extracts the default pages from the embedded resources.
/// This allows users to customize error pages by overriding these files.
pub fn ensure_status_pages_exist() -> Result<()> {
    let (_, config_dir) = config::get_config_paths()?;
    let status_pages_dir = config_dir.join("status");

    if !status_pages_dir.exists() {
        log(
            LogLevel::Info,
            "Status pages directory not found. Creating default pages...",
        );
        fs::create_dir_all(&status_pages_dir).context("Failed to create status pages directory")?;

        // Iterate through the embedded directory and write each file to disk
        for file in STATIC_STATUS_PAGES.files() {
            let path = status_pages_dir.join(file.path());
            fs::write(&path, file.contents())
                .with_context(|| format!("Failed to write status page: {:?}", path))?;
        }
    }

    Ok(())
}

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

    // Also ensure status pages are created during the first run.
    ensure_status_pages_exist()?;

    log(
        LogLevel::Info,
        "First-time setup complete. Please start Vane again.",
    );
    Ok(())
}

/// Generates a self-signed certificate and private key using the rcgen crate.
fn generate_self_signed_cert(hostname: &str, cert_path: &Path, key_path: &Path) -> Result<()> {
    let cert = generate_simple_self_signed(vec![hostname.to_string()])?;
    fs::write(cert_path, cert.cert.pem())?;
    fs::write(key_path, cert.signing_key.serialize_pem())?;
    Ok(())
}
