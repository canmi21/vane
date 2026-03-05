/* src/app/src/l7/http/h3.rs */

use super::wrapper::{H3BodyAdapter, VaneBody};
use crate::l7::{
	container::{Container, PayloadState},
	flow,
};
use fancy_log::{LogLevel, log};
use vane_primitives::common::sys::lifecycle::{Error, Result};
use vane_primitives::kv::KvStore;

use bytes::{Buf, Bytes};
use h3::server::RequestStream;
use h3_quinn::quinn::Connection;
use http::{HeaderMap, Request, Response};
use http_body_util::BodyExt;
use tokio::sync::{mpsc, oneshot};

pub async fn handle_connection(quic_conn: Connection) -> Result<()> {
	log(LogLevel::Debug, "➜ Starting L7 H3 Engine...");

	let h3_quinn_conn = h3_quinn::Connection::new(quic_conn);
	let mut h3_conn: h3::server::Connection<h3_quinn::Connection, bytes::Bytes> =
		match h3::server::Connection::new(h3_quinn_conn).await {
			Ok(driver) => driver,
			Err(e) => {
				return Err(Error::System(format!("H3 Protocol Handshake failed: {e}")));
			}
		};

	loop {
		match h3_conn.accept().await {
			Ok(Some(resolver)) => {
				log(LogLevel::Debug, "➜ Received new request resolver");
				tokio::spawn(async move {
					match resolver.resolve_request().await {
						Ok((req, stream)) => {
							if let Err(e) = serve_h3_request(req, stream).await {
								log(LogLevel::Error, &format!("✗ H3 Request Error: {e:#}"));
							}
						}
						Err(e) => {
							log(LogLevel::Error, &format!("✗ Failed to resolve request: {e}"));
						}
					}
				});
			}
			Ok(None) => break,
			Err(e) => {
				log(LogLevel::Warn, &format!("⚠ H3 Accept Error: {e}"));
				break;
			}
		}
	}

	Ok(())
}

// Removed generic B, hardcoded to bytes::Bytes.
#[allow(clippy::too_many_lines)]
async fn serve_h3_request<T>(
	req: Request<()>,
	mut stream: RequestStream<T, bytes::Bytes>,
) -> anyhow::Result<()>
where
	T: h3::quic::BidiStream<bytes::Bytes> + Send + Unpin + 'static,
{
	let (mut parts, _) = req.into_parts();

	// 1. Request Body Pump Channel (Client -> Container)
	let (body_tx, body_rx) = mpsc::channel::<Result<Bytes>>(32);
	let mut body_tx = Some(body_tx);

	// 2. Response Signal Channel (Container -> Driver)
	let (res_tx, res_rx) = oneshot::channel::<Response<()>>();

	// Container Construction
	let adapter = H3BodyAdapter::new(body_rx);
	let boxed_body = adapter.map_err(|e| e).boxed();
	let request_payload = PayloadState::Http(VaneBody::H3(boxed_body));
	let response_payload = PayloadState::Empty;

	let mut kv = KvStore::new();
	kv.insert("req.proto".to_owned(), "h3".to_owned());
	kv.insert("req.method".to_owned(), parts.method.to_string());
	kv.insert("req.path".to_owned(), parts.uri.path().to_owned());

	// Inject Query String
	if let Some(q) = parts.uri.query() {
		kv.insert("req.query".to_owned(), q.to_owned());
	}

	if let Some(host) = parts.headers.get("host").or_else(|| parts.headers.get(":authority"))
		&& let Ok(h) = host.to_str()
	{
		kv.insert("req.host".to_owned(), h.to_owned());
	}

	let request_headers = std::mem::take(&mut parts.headers);
	let response_headers = HeaderMap::new();

	let mut container = Container::new(
		kv,
		request_headers,
		request_payload,
		response_headers,
		response_payload,
		Some(res_tx),
	);

	let config = {
		let config_manager = vane_engine::config::get();
		config_manager
			.applications
			.get("h3")
			.or_else(|| config_manager.applications.get("httpx"))
			.ok_or_else(|| anyhow::anyhow!("No application config found for 'h3' or 'httpx'"))?
	};

	// Spawn Flow Execution (Consumer of Request Body, Producer of Response)
	// We need to retrieve the response BODY stream from the container.
	let flow_handle = tokio::spawn(async move {
		if let Err(e) = flow::execute_l7(&config.pipeline, &mut container, String::new()).await {
			log(LogLevel::Error, &format!("✗ L7 Flow Logic Failed: {e:#}"));
			return None;
		}
		// Extract Response Body BEFORE container drops
		// Note: ensure httpx::extract_response_body_from_container is pub(super)
		let body = super::httpx::extract_response_body_from_container(&mut container);
		Some(body)
	});

	// --- The Driver Loop (Bidirectional) ---
	let mut res_rx = res_rx; // Wait for headers

	// Wrap handle in Option to allow taking ownership inside loop
	let mut flow_task = Some(flow_handle);

	let mut response_body_stream: Option<http_body_util::combinators::BoxBody<Bytes, Error>> = None;

	let mut request_finished = false;
	let mut response_finished = false;

	loop {
		if request_finished && response_finished {
			break;
		}

		tokio::select! {
			// Branch A: Pump Request Body (Stream -> Channel)
			recv_result = stream.recv_data(), if !request_finished => {
				match recv_result {
					Ok(Some(mut buf)) => {
						let bytes = buf.copy_to_bytes(buf.remaining());
						if let Some(tx) = body_tx.as_ref()
							&& tx.send(Ok(bytes)).await.is_err() {
								request_finished = true;
								body_tx = None;
							}
					}
					Ok(None) => {
						// EOF
						request_finished = true;
						body_tx = None;
					}
					Err(e) => {
						if let Some(tx) = body_tx.as_ref() {
							let _ = tx.send(Err(Error::System(e.to_string()))).await;
						}
						request_finished = true;
						body_tx = None;
					}
				}
			}

			// Branch B: Wait for Response Headers
			res_signal = &mut res_rx, if response_body_stream.is_none() && !response_finished => {
				if let Ok(response) = res_signal {
								if let Err(e) = stream.send_response(response).await {
									log(LogLevel::Error, &format!("✗ Failed to send H3 headers: {e}"));
									response_finished = true;
								}

								// Take ownership of the task handle
								if let Some(task) = flow_task.take() {
									if let Ok(Some(body)) = task.await {
										response_body_stream = Some(body);
									} else {
										response_finished = true;
										let _ = stream.finish().await;
									}
								} else {
									// Should not happen if logic flows correctly
									response_finished = true;
									let _ = stream.finish().await;
								}
							} else {
								// Flow failed or dropped sender
								response_finished = true;
								let _ = stream.finish().await;
							}
			}

			// Branch C: Pump Response Body (Stream -> H3)
			frame_future = async {
				if let Some(b) = response_body_stream.as_mut() {
					b.frame().await
				} else {
					std::future::pending().await
				}
			}, if response_body_stream.is_some() && !response_finished => {
				match frame_future {
					Some(Ok(frame)) => {
						if let Ok(data) = frame.into_data()
							&& !data.is_empty()
								&& let Err(e) = stream.send_data(data).await {
									log(LogLevel::Warn, &format!("Failed to send H3 data: {e}"));
									response_finished = true;
								}
					}
					Some(Err(e)) => {
						log(LogLevel::Error, &format!("Response Body Error: {e}"));
						response_finished = true;
						let _ = stream.finish().await;
					}
					None => {
						// End of Response Stream
						response_finished = true;
						let _ = stream.finish().await;
					}
				}
			}
		}
	}

	Ok(())
}
