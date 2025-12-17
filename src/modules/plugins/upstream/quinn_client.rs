/* src/modules/plugins/upstream/quinn_client.rs */

use super::quic_pool;
use crate::common::requirements::{Error, Result};
use crate::modules::stack::protocol::application::{
	container::{Container, PayloadState},
	http::wrapper::VaneBody,
};
use bytes::Buf;
use fancy_log::{LogLevel, log};
use http::{Request, Uri};
use http_body_util::BodyExt;
use std::str::FromStr;

pub async fn execute_quinn_request(
	container: &mut Container,
	url_str: &str,
	method_str: Option<&str>,
	skip_verify: bool,
) -> Result<()> {
	// 1. Parse URI & Address
	let uri =
		Uri::from_str(url_str).map_err(|e| Error::Configuration(format!("Invalid URL: {}", e)))?;

	let host = uri
		.host()
		.ok_or_else(|| Error::Configuration("URL missing host".into()))?;
	let port = uri.port_u16().unwrap_or(443);

	// 2. Get Connection from Pool (Reuse or Create)
	// This step is now instant if connection exists
	let mut send_request = quic_pool::get_or_create_connection(host, port, skip_verify).await?;

	// 3. Determine Method
	let req_method = if let Some(m) = method_str {
		http::Method::from_str(m).unwrap_or(http::Method::GET)
	} else {
		container
			.kv
			.get("req.method")
			.and_then(|m| http::Method::from_str(m).ok())
			.unwrap_or(http::Method::GET)
	};

	// 4. Build Request
	let mut req = Request::builder().method(req_method).uri(uri);

	// HEADER PROPAGATION
	if let Some(headers) = req.headers_mut() {
		*headers = container.request_headers.clone();
		headers.remove(http::header::HOST);
	}

	let request = req.body(()).unwrap();

	// 5. Send Headers
	let mut stream = send_request
		.send_request(request)
		.await
		.map_err(|e| Error::System(format!("Failed to send H3 headers: {}", e)))?;

	// 6. Pump Request Body
	let req_payload = std::mem::replace(&mut container.request_body, PayloadState::Empty);

	let body_handle = tokio::spawn(async move {
		match req_payload {
			PayloadState::Http(mut vane_body) => {
				while let Some(frame) = vane_body.frame().await {
					match frame {
						Ok(f) => {
							if let Ok(data) = f.into_data() {
								if !data.is_empty() {
									if stream.send_data(data).await.is_err() {
										break;
									}
								}
							}
						}
						Err(_) => break,
					}
				}
				let _ = stream.finish().await;
			}
			PayloadState::Buffered(bytes) => {
				if !bytes.is_empty() {
					let _ = stream.send_data(bytes).await;
				}
				let _ = stream.finish().await;
			}
			_ => {
				let _ = stream.finish().await;
			}
		}
		stream
	});

	// 7. Receive Response
	let mut stream = body_handle
		.await
		.map_err(|e| Error::System(format!("Body pump task failed: {}", e)))?;

	let response = stream
		.recv_response()
		.await
		.map_err(|e| Error::System(format!("Failed to receive H3 response: {}", e)))?;

	let status = response.status();
	log(
		LogLevel::Debug,
		&format!("✓ H3 Upstream Responded: {}", status),
	);

	container
		.kv
		.insert("res.status".to_string(), status.as_u16().to_string());
	container.response_headers = response.headers().clone();

	// 8. Stream Response Body
	let (body_tx, body_rx) = tokio::sync::mpsc::channel(32);

	tokio::spawn(async move {
		while let Ok(Some(mut chunk)) = stream.recv_data().await {
			let bytes = chunk.copy_to_bytes(chunk.remaining());
			if body_tx.send(Ok(bytes)).await.is_err() {
				break;
			}
		}
	});

	let adapter =
		crate::modules::stack::protocol::application::http::wrapper::H3BodyAdapter::new(body_rx);
	container.response_body = PayloadState::Http(VaneBody::H3(adapter.boxed()));

	Ok(())
}
