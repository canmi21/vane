/* src/modules/plugins/drivers/unix.rs */

use crate::modules::plugins::model::{ExternalApiResponse, MiddlewareOutput, ResolvedInputs};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub async fn execute(path: &str, name: &str, inputs: ResolvedInputs) -> Result<MiddlewareOutput> {
	log(
		LogLevel::Debug,
		&format!("➜ Executing external Unix middleware: {}", name),
	);

	// 1. Connect to Unix Socket
	let mut stream = UnixStream::connect(path)
		.await
		.map_err(|e| anyhow!("Failed to connect to unix socket {}: {}", path, e))?;

	// 2. Serialize Payload
	let body_bytes = serde_json::to_vec(&inputs)?;
	let body_len = body_bytes.len();

	// 3. Construct Raw HTTP/1.1 Request
	// Note: Host header is required by HTTP/1.1 but ignored by UDS usually.
	let request_header = format!(
		"POST / HTTP/1.1\r\n\
        Host: localhost\r\n\
        Content-Type: application/json\r\n\
        Content-Length: {}\r\n\
        Connection: close\r\n\
        \r\n",
		body_len
	);

	// 4. Write Request
	stream.write_all(request_header.as_bytes()).await?;
	stream.write_all(&body_bytes).await?;
	stream.flush().await?;

	// 5. Read Response
	// For simplicity in this lightweight driver, we read until EOF (Connection: close).
	let mut response_bytes = Vec::new();
	stream.read_to_end(&mut response_bytes).await?;

	if response_bytes.is_empty() {
		return Err(anyhow!("External Unix plugin returned empty response."));
	}

	// 6. Parse HTTP Response (Simplified)
	// We need to find the double CRLF to separate headers from body.
	let response_str = String::from_utf8_lossy(&response_bytes);
	let mut parts = response_str.splitn(2, "\r\n\r\n");

	let _headers_part = parts
		.next()
		.ok_or_else(|| anyhow!("Invalid HTTP response format"))?;
	let body_part = parts
		.next()
		.ok_or_else(|| anyhow!("HTTP response body missing"))?;

	// Optional: Check HTTP status line (e.g., "HTTP/1.1 200 OK")
	// For now, we assume if we got a JSON body, we parse the logical status inside.

	// 7. Parse Body as ExternalApiResponse
	let api_response: ExternalApiResponse<MiddlewareOutput> = serde_json::from_str(body_part)
		.map_err(|e| anyhow!("Failed to parse external API response JSON: {}", e))?;

	// 8. Check Logic Status
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
