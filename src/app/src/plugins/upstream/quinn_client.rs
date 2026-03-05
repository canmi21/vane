use super::quic_pool;
use crate::l7::{
	container::{Container, PayloadState},
	http::wrapper::{H3BodyAdapter, VaneBody},
};
use bytes::Buf;
use fancy_log::{LogLevel, log};
use http::{Request, Uri};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use std::str::FromStr;
use tokio::sync::mpsc;
use vane_primitives::common::sys::lifecycle::{Error, Result};

/// H3 Upstream Client Driver (Fully Async & Non-Blocking)
///
/// Refactored to avoid Deadlocks during full-duplex streaming (e.g. 1GB Echo tests).
/// We spawn separated tasks for Request Pumping and Response Pumping.
pub async fn execute_quinn_request(
	container: &mut Container,
	url_str: &str,
	method_str: Option<&str>,
	skip_verify: bool,
) -> Result<()> {
	let uri =
		Uri::from_str(url_str).map_err(|e| Error::Configuration(format!("Invalid URL: {e}")))?;

	let host = uri
		.host()
		.ok_or_else(|| Error::Configuration("URL missing host".into()))?;
	let port = uri.port_u16().unwrap_or(443);

	// 1. Get Connection
	let mut send_request = quic_pool::get_or_create_connection(host, port, skip_verify).await?;

	let req_method = if let Some(m) = method_str {
		http::Method::from_str(m).unwrap_or(http::Method::GET)
	} else {
		container
			.kv
			.get("req.method")
			.and_then(|m| http::Method::from_str(m).ok())
			.unwrap_or(http::Method::GET)
	};

	// 2. Prepare Headers
	let mut req = Request::builder().method(req_method).uri(uri);
	if let Some(headers) = req.headers_mut() {
		*headers = container.request_headers.clone();
		headers.remove(http::header::HOST);
	}
	let request = req.body(()).unwrap();

	// 3. Open Stream & Send Headers
	let stream = send_request
		.send_request(request)
		.await
		.map_err(|e| Error::System(format!("Failed to send H3 headers: {e}")))?;

	// Split stream for concurrent Read/Write to avoid head-of-line blocking/deadlocks
	let (mut driver_send, mut driver_recv) = stream.split();

	// 4. Prepare Request Body Stream
	let req_payload = std::mem::replace(&mut container.request_body, PayloadState::Empty);
	let req_body_stream: Option<BoxBody<bytes::Bytes, Error>> = match req_payload {
		PayloadState::Http(vane_body) => Some(vane_body.boxed()),
		PayloadState::Buffered(bytes, _guard) => Some(Full::new(bytes).map_err(|e| match e {}).boxed()),
		_ => None,
	};

	// 5. SPAWN UPLOAD TASK (Request Body -> Upstream)
	// This ensures that writing to the upstream doesn't block waiting for response headers.
	if let Some(mut body) = req_body_stream {
		tokio::spawn(async move {
			loop {
				match body.frame().await {
					Some(Ok(frame)) => {
						if let Ok(data) = frame.into_data()
							&& !data.is_empty()
							&& let Err(e) = driver_send.send_data(data).await
						{
							log(LogLevel::Warn, &format!("⚠ H3 Upload interrupted: {e}"));
							break;
						}
					}
					Some(Err(e)) => {
						log(LogLevel::Error, &format!("✗ H3 Request Read Error: {e}"));
						break;
					}
					None => {
						// EOF
						break;
					}
				}
			}
			let _ = driver_send.finish().await;
		});
	} else {
		// No body, finish immediately
		let _ = driver_send.finish().await;
	}

	// 6. Await Response Headers (Main Task)
	// This might happen while upload is still active (Full Duplex)
	let response = match driver_recv.recv_response().await {
		Ok(res) => res,
		Err(e) => {
			return Err(Error::System(format!("Failed to receive H3 response: {e}")));
		}
	};

	let status = response.status();
	log(
		LogLevel::Debug,
		&format!("✓ H3 Upstream Responded: {status}"),
	);

	// 7. Update Container Headers
	container
		.kv
		.insert("res.status".to_owned(), status.as_u16().to_string());
	container.response_headers = response.headers().clone();

	// 8. SPAWN DOWNLOAD TASK (Upstream -> Channel -> Response Body)
	let (res_body_tx, res_body_rx) = mpsc::channel::<Result<bytes::Bytes>>(32);

	tokio::spawn(async move {
		loop {
			match driver_recv.recv_data().await {
				Ok(Some(mut chunk)) => {
					let bytes = chunk.copy_to_bytes(chunk.remaining());
					if res_body_tx.send(Ok(bytes)).await.is_err() {
						// Downstream receiver dropped, stop reading upstream
						driver_recv.stop_sending(h3::error::Code::H3_REQUEST_CANCELLED);
						break;
					}
				}
				Ok(None) => {
					// EOF
					break;
				}
				Err(e) => {
					log(LogLevel::Error, &format!("✗ H3 Download Error: {e}"));
					let _ = res_body_tx.send(Err(Error::System(e.to_string()))).await;
					break;
				}
			}
		}
	});

	// 9. Install Response Body Adapter
	let adapter = H3BodyAdapter::new(res_body_rx);
	container.response_body = PayloadState::Http(VaneBody::H3(adapter.boxed()));

	Ok(())
}
