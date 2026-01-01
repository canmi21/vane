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
	let timeout_secs = getenv::get_env("FLOW_EXECUTION_TIMEOUT_SECS", "10".to_string())
		.parse::<u64>()
		.unwrap_or(10);

	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(timeout_secs)) // Runtime timeout
		.danger_accept_invalid_certs(skip_tls)
		.build()
		.map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;

	// 3. Send POST Request
	let response = match client.post(url).json(&inputs).send().await {
		Ok(r) => r,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ External HTTP request failed for '{}': {}", name, e),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	// 4. Validate HTTP Status
	if !response.status().is_success() {
		log(
			LogLevel::Error,
			&format!(
				"✗ External plugin '{}' returned HTTP error: {}",
				name,
				response.status()
			),
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	}

	// 5. Parse Response Wrapper (ExternalApiResponse)
	let api_response: ExternalApiResponse<MiddlewareOutput> = match response.json().await {
		Ok(r) => r,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to parse external API response JSON for '{}': {}",
					name, e
				),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	// 6. Check Logic Status
	if api_response.status == "success" {
		api_response.data.ok_or_else(|| {
			anyhow!(
				"External API for '{}' returned success but 'data' is missing.",
				name
			)
		})
	} else {
		let msg = api_response
			.message
			.unwrap_or_else(|| "Unknown error".to_string());
		log(
			LogLevel::Warn,
			&format!(
				"⚠ External API for '{}' returned error status: {}",
				name, msg
			),
		);
		Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		})
	}
}
