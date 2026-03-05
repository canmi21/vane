/* src/plugins/system/unix.rs */

use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;
use vane_engine::engine::interfaces::{ExternalApiResponse, MiddlewareOutput, ResolvedInputs};

pub async fn execute(path: &str, name: &str, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
	let timeout_secs = envflag::get::<u64>("FLOW_EXECUTION_TIMEOUT_SECS", 10);
	let duration = Duration::from_secs(timeout_secs);

	log(
		LogLevel::Debug,
		&format!("➜ Executing external Unix middleware (timeout {timeout_secs}s): {name}"),
	);

	// 1. Connect to Unix Socket
	let Ok(res) = timeout(duration, UnixStream::connect(path)).await else {
		log(
			LogLevel::Error,
			&format!("✗ Unix connection to {path} timed out after {timeout_secs}s"),
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	};

	let mut stream = match res {
		Ok(s) => s,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to connect to unix socket {path}: {e}"),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	// 2. Serialize Payload
	let body_bytes = serde_json::to_vec(&inputs)?;
	let body_len = body_bytes.len();

	// 3. Construct Raw HTTP/1.1 Request
	let request_header = format!(
		"POST / HTTP/1.1\r\n\
        Host: localhost\r\n\
        Content-Type: application/json\r\n\
        Content-Length: {body_len}\r\n\
        Connection: close\r\n\
        \r\n"
	);

	// 4. Write Request
	if let Err(e) = stream.write_all(request_header.as_bytes()).await {
		log(
			LogLevel::Error,
			&format!("✗ Failed to write header to unix socket: {e}"),
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	}
	if let Err(e) = stream.write_all(&body_bytes).await {
		log(
			LogLevel::Error,
			&format!("✗ Failed to write body to unix socket: {e}"),
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	}
	let _ = stream.flush().await;

	// 5. Read Response
	let mut response_bytes = Vec::new();
	if let Err(e) = timeout(duration, stream.read_to_end(&mut response_bytes)).await {
		log(
			LogLevel::Error,
			&format!("✗ Unix read from {path} timed out or failed: {e}"),
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	}

	if response_bytes.is_empty() {
		log(
			LogLevel::Error,
			"✗ External Unix plugin returned empty response.",
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	}

	// 6. Parse HTTP Response (Simplified)
	let response_str = String::from_utf8_lossy(&response_bytes);
	let mut parts = response_str.splitn(2, "\r\n\r\n");

	let _headers_part = parts.next();
	let Some(body_part) = parts.next() else {
		log(
			LogLevel::Error,
			"✗ HTTP response body missing from unix socket.",
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	};

	// 7. Parse Body as ExternalApiResponse
	let api_response: ExternalApiResponse<MiddlewareOutput> = match serde_json::from_str(body_part) {
		Ok(r) => r,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to parse API response JSON: {e}"),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	// 8. Check Logic Status
	if api_response.status == "success" {
		api_response
			.data
			.ok_or_else(|| anyhow!("External API for '{name}' returned success but 'data' is missing."))
	} else {
		let msg = api_response
			.message
			.unwrap_or_else(|| "Unknown error".to_owned());
		log(
			LogLevel::Warn,
			&format!("⚠ External API for '{name}' returned error status: {msg}"),
		);
		Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		})
	}
}
