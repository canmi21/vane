/* src/acme_client.rs */

use anyhow::{Context, Result, anyhow};
use fancy_log::{LogLevel, log};
use serde::Deserialize;
use std::{fs, path::Path, time::Duration};

// Structs to deserialize the JSON response from the lazy-acme server.
#[derive(Deserialize)]
struct ApiResponseData {
    certificate_base64: Option<String>,
    key_base64: Option<String>,
}

#[derive(Deserialize)]
struct ApiResponse {
    status: String,
    data: Option<ApiResponseData>,
}

const RETRY_ATTEMPTS: u32 = 5;
const RETRY_DELAY_SECONDS: u64 = 5;

/// Fetches a certificate and its private key from the CERT_SERVER and saves them to disk.
pub async fn fetch_and_save_certificate(
    domain: &str,
    server_url: &str,
    cert_path: &Path,
    key_path: &Path,
) -> Result<()> {
    log(
        LogLevel::Info,
        &format!(
            "Attempting to fetch certificate for '{}' from ACME server...",
            domain
        ),
    );

    // Fetch the certificate chain
    let cert_pem = fetch_resource(domain, server_url, "certificate").await?;
    // Fetch the private key
    let key_pem = fetch_resource(domain, server_url, "key").await?;

    // Create the parent directory if it doesn't exist.
    if let Some(parent) = cert_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create cert directory at {:?}", parent))?;
    }

    fs::write(cert_path, cert_pem)
        .with_context(|| format!("Failed to write certificate to {:?}", cert_path))?;
    fs::write(key_path, key_pem)
        .with_context(|| format!("Failed to write private key to {:?}", key_path))?;

    log(
        LogLevel::Info,
        &format!(
            "Successfully fetched and saved certificate for '{}'.",
            domain
        ),
    );

    Ok(())
}

/// Helper function to fetch a resource (cert or key) with retry logic.
async fn fetch_resource(domain: &str, server_url: &str, resource_type: &str) -> Result<String> {
    let endpoint = match resource_type {
        "certificate" => format!("{}/v1/certificate/{}", server_url, domain),
        "key" => format!("{}/v1/certificate/{}/key", server_url, domain),
        _ => return Err(anyhow!("Invalid resource type requested")),
    };

    for attempt in 1..=RETRY_ATTEMPTS {
        log(
            LogLevel::Debug,
            &format!(
                "Fetching {} for '{}' (Attempt {}/{})",
                resource_type, domain, attempt, RETRY_ATTEMPTS
            ),
        );

        match reqwest::get(&endpoint).await {
            Ok(response) => {
                if response.status().is_success() {
                    let api_response: ApiResponse = response
                        .json()
                        .await
                        .context("Failed to parse JSON response from ACME server")?;

                    if api_response.status.to_lowercase() == "success" {
                        if let Some(data) = api_response.data {
                            let base64_str = match resource_type {
                                "certificate" => data.certificate_base64,
                                "key" => data.key_base64,
                                _ => None,
                            };

                            if let Some(encoded) = base64_str {
                                use base64::{Engine as _, engine::general_purpose};
                                let decoded_bytes = general_purpose::STANDARD.decode(encoded)?;
                                return Ok(String::from_utf8(decoded_bytes)?);
                            }
                        }
                    }
                    return Err(anyhow!(
                        "ACME server returned success status but response data was invalid."
                    ));
                } else if response.status() == reqwest::StatusCode::NOT_FOUND {
                    return Err(anyhow!(
                        "ACME server returned 404 Not Found for domain '{}'. Certificate does not exist.",
                        domain
                    ));
                } else {
                    // Log other HTTP errors but continue to retry
                    log(
                        LogLevel::Warn,
                        &format!(
                            "ACME server returned non-success status: {}. Retrying...",
                            response.status()
                        ),
                    );
                }
            }
            Err(e) => {
                // Log connection errors but continue to retry
                log(
                    LogLevel::Warn,
                    &format!("Failed to connect to ACME server: {}. Retrying...", e),
                );
            }
        }
        // Wait before the next attempt
        tokio::time::sleep(Duration::from_secs(RETRY_DELAY_SECONDS)).await;
    }

    Err(anyhow!(
        "Failed to fetch {} for '{}' after {} attempts.",
        resource_type,
        domain,
        RETRY_ATTEMPTS
    ))
}
