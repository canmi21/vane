/* src/modules/plugins/drivers/httpx.rs */

use crate::{
	common::getenv,
	modules::plugins::model::{ExternalApiResponse, MiddlewareOutput, ResolvedInputs},
};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use std::time::Duration;

pub async fn execute(url: &str, name: &str, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
	log(
		LogLevel::Debug,
		&format!("➜ Executing external HTTP middleware: {}", name),
	);

	// 1. Check Env for TLS Verification Skip
	let skip_tls = getenv::to_lowercase(&getenv::get_env(
		"EXTERNAL_HTTPS_CALL_SKIP_TLS_VERIFY",
		"false".to_string(),
	)) == "true";

	if skip_tls {
		log(
			LogLevel::Debug,
			&format!(
				"⚠ TLS Verification disabled for external plugin '{}' via EXTERNAL_HTTPS_CALL_SKIP_TLS_VERIFY.",
				name
			),
		);
	}

	// 2. Build Client
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(10)) // Runtime timeout
		.danger_accept_invalid_certs(skip_tls)
		.build()
		.map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;

	// 3. Send POST Request
	let response = client
		.post(url)
		.json(&inputs)
		.send()
		.await
		.map_err(|e| anyhow!("External HTTP request failed: {}", e))?;

	// 4. Validate HTTP Status
	if !response.status().is_success() {
		return Err(anyhow!(
			"External plugin returned HTTP error: {}",
			response.status()
		));
	}

	// 5. Parse Response Wrapper (ExternalApiResponse)
	let api_response: ExternalApiResponse<MiddlewareOutput> = response
		.json()
		.await
		.map_err(|e| anyhow!("Failed to parse external API response JSON: {}", e))?;

	// 6. Check Logic Status
	if api_response.status == "success" {
		api_response
			.data
			.ok_or_else(|| anyhow!("External API returned success but 'data' is missing."))
	} else {
		let msg = api_response
			.message
			.unwrap_or_else(|| "Unknown error".to_string());
		Err(anyhow!("External API returned error: {}", msg))
	}
}
