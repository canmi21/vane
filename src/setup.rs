/* src/setup.rs */

use crate::{acme_client, config};
use anyhow::{Context, Result};
use fancy_log::{LogLevel, log};
use include_dir::{Dir, include_dir};
use rcgen::generate_simple_self_signed;
use std::{env, fs, path::Path};

// Embeds the `./status` directory into the binary at compile time.
static STATIC_STATUS_PAGES: Dir = include_dir!("$CARGO_MANIFEST_DIR/status");

const DEFAULT_MAIN_CONFIG: &str = r#"
# Vane main configuration file
# This file maps hostnames to their specific configuration files.
[domains]
"example.com" = "example.com.toml"
"#;

// MODIFIED: Added detailed comments for every configuration option.
const DEFAULT_DOMAIN_CONFIG: &str = r#"
# Vane domain configuration for example.com

# --- Core Protocol Settings ---
# Enable HTTPS on the standard port (443 by default).
https = true
# Enable HTTP/3 over QUIC on the HTTPS UDP port. Requires `https` to be true.
http3 = true
# Enable HSTS (HTTP Strict Transport Security) header to enforce HTTPS on clients.
hsts = true
# Behavior for plain HTTP requests on port 80:
# "upgrade" (redirects to HTTPS), "reject" (blocks), or "allow".
http_options = "upgrade"

# --- TLS Certificate Settings ---
[tls]
# Path to the PEM-encoded TLS certificate file. Supports '~' for the home directory.
cert = "~/vane/certs/example.com.pem"
# Path to the PEM-encoded private key file. Supports '~' for the home directory.
key = "~/vane/certs/example.com.key"

# --- Method Filtering ---
# Optional: Restrict which HTTP methods are allowed for this entire domain.
# This check happens before CORS or routing. Use "*" to allow all methods.
[methods]
allow = "GET, POST, OPTIONS, HEAD"

# --- CORS (Cross-Origin Resource Sharing) ---
# Optional: Fine-grained CORS configuration.
# If this section is present, Vane will override any CORS headers from the backend.
[cors]
# Map of allowed origins to their allowed methods.
[cors.origins]
# For methods, use a comma-separated string (e.g., "GET, POST"), or use "*" to allow all methods from that origin.
"https://app.example.com" = "GET, POST, OPTIONS"
"http://localhost:3000" = "*"

# --- Rate Limiting ---
[rate_limit]
# Default rate limit applied to all requests for this domain unless a more specific rule matches.
[rate_limit.default]
# The time window for the rate limit (e.g., "1s", "10m", "1h").
period = "1s"
# Number of requests allowed in the period. Set to 0 to disable.
requests = 20

# --- Routing Rules ---
# Define how incoming paths are proxied to backend targets.
# Rules are matched from top to bottom.
[[routes]]
# The URL path to match. Supports wildcards (*) at the end.
path = "/api/*"
# A list of backend servers. Vane will try them in order.
# If the first target fails (connection error or 5xx response), it will try the second, and so on.
targets = ["http://12.0.0.1:8000", "http://127.0.0.1:8001"] # Primary and fallback targets

[[routes]]
path = "/"
targets = ["http://127.0.0.1:33433"]
"#;

/// Checks if the status pages directory exists and creates it if not.
pub fn ensure_status_pages_exist() -> Result<()> {
    let (_, config_dir) = config::get_config_paths()?;
    let status_pages_dir = config_dir.join("status");

    if !status_pages_dir.exists() {
        log(
            LogLevel::Info,
            "Status pages directory not found. Creating default pages...",
        );
        fs::create_dir_all(&status_pages_dir).context("Failed to create status pages directory")?;

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
    let cert_dir_str = env::var("CERT_DIR").unwrap_or_else(|_| "~/vane/certs".to_string());

    // --- MODIFICATION START ---
    // Create a longer-lived String to satisfy the borrow checker.
    let expanded_cert_dir = shellexpand::tilde(&cert_dir_str).into_owned();
    let certs_dir = Path::new(&expanded_cert_dir);
    // --- MODIFICATION END ---

    fs::create_dir_all(&certs_dir).context("Failed to create certs directory")?;

    let cert_path = certs_dir.join("example.com.pem");
    let key_path = certs_dir.join("example.com.key");

    if !cert_path.exists() || !key_path.exists() {
        // Check if CERT_SERVER is set.
        if let Ok(server_url) = env::var("CERT_SERVER") {
            // If CERT_SERVER is set, try to fetch a real certificate.
            if let Err(e) = acme_client::fetch_and_save_certificate(
                "example.com",
                &server_url,
                &cert_path,
                &key_path,
            )
            .await
            {
                log(
                    LogLevel::Error,
                    &format!("Failed to fetch initial certificate for example.com: {}", e),
                );
                log(
                    LogLevel::Error,
                    "Please ensure lazy-acme server is running and the domain is configured. Vane will exit.",
                );
                // Use a specific exit code to indicate cert failure.
                std::process::exit(75); // EX_TEMPFAIL
            }
        } else {
            // Otherwise, fall back to self-signing.
            log(
                LogLevel::Info,
                "CERT_SERVER not set. Generating self-signed certificate for example.com...",
            );
            generate_self_signed_cert("example.com", &cert_path, &key_path)?;
        }
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

    ensure_status_pages_exist()?;

    log(
        LogLevel::Info,
        "First-time setup complete. Please start Vane again.",
    );
    Ok(())
}

/// Generates a self-signed certificate and private key.
fn generate_self_signed_cert(hostname: &str, cert_path: &Path, key_path: &Path) -> Result<()> {
    let cert = generate_simple_self_signed(vec![hostname.to_string()])?;
    fs::write(cert_path, cert.cert.pem())?;
    fs::write(key_path, cert.signing_key.serialize_pem())?;
    Ok(())
}
