/* src/modules/stack/protocol/application/http/h3.rs */

use super::wrapper::{H3BodyAdapter, VaneBody};
use crate::common::requirements::{Error, Result};
use crate::modules::kv::KvStore;
use crate::modules::stack::protocol::application::{
	container::{Container, PayloadState},
	flow,
	model::APPLICATION_REGISTRY,
};
use fancy_log::{LogLevel, log};

use bytes::{Buf, Bytes};
use h3::server::RequestStream;
use h3_quinn::quinn::Connection;
use http::{Request, Response};
use http_body_util::BodyExt;
use tokio::sync::{mpsc, oneshot};

pub async fn handle_connection(quic_conn: Connection) -> Result<()> {
	log(LogLevel::Debug, "➜ Starting L7 H3 Engine...");

	let h3_quinn_conn = h3_quinn::Connection::new(quic_conn);
	let mut h3_conn: h3::server::Connection<h3_quinn::Connection, bytes::Bytes> =
		match h3::server::Connection::new(h3_quinn_conn).await {
			Ok(driver) => driver,
			Err(e) => {
				return Err(Error::System(format!(
					"H3 Protocol Handshake failed: {}",
					e
				)));
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
								log(LogLevel::Error, &format!("✗ H3 Request Error: {:#}", e));
							}
						}
						Err(e) => {
							log(
								LogLevel::Error,
								&format!("✗ Failed to resolve request: {}", e),
							);
						}
					}
				});
			}
			Ok(None) => break,
			Err(e) => {
				log(LogLevel::Warn, &format!("⚠ H3 Accept Error: {}", e));
				break;
			}
		}
	}

	Ok(())
}

async fn serve_h3_request<T, B>(
	req: Request<()>,
	mut stream: RequestStream<T, B>,
) -> anyhow::Result<()>
where
	T: h3::quic::BidiStream<B> + Send + Unpin + 'static,
	B: Buf + Send + 'static,
{
	let (parts, _) = req.into_parts();

	// Infrastructure Setup Channels
	let (body_tx, body_rx) = mpsc::channel::<Result<Bytes>>(32);
	let mut body_tx = Some(body_tx);

	let (res_tx, res_rx) = oneshot::channel::<Response<()>>();
	tokio::pin!(res_rx);

	// Container Construction
	let adapter = H3BodyAdapter::new(body_rx);
	let boxed_body = adapter.map_err(|e| e).boxed();

	// Assign H3 Body to REQUEST slot
	let request_payload = PayloadState::Http(VaneBody::H3(boxed_body));
	// Initialize RESPONSE slot as Empty
	let response_payload = PayloadState::Empty;

	let mut kv = KvStore::new();
	kv.insert("req.proto".to_string(), "h3".to_string());
	kv.insert("req.method".to_string(), parts.method.to_string());
	kv.insert("req.path".to_string(), parts.uri.path().to_string());

	if let Some(host) = parts
		.headers
		.get("host")
		.or_else(|| parts.headers.get(":authority"))
	{
		if let Ok(h) = host.to_str() {
			kv.insert("req.host".to_string(), h.to_string());
		}
	}

	let mut container = Container::new(kv, request_payload, response_payload, Some(res_tx));

	// Logic Execution Spawned

	let config = {
		let registry = APPLICATION_REGISTRY.load();
		registry
			.get("h3")
			.or_else(|| registry.get("httpx"))
			.map(|entry| entry.value().clone())
			.ok_or_else(|| anyhow::anyhow!("No application config found for 'h3' or 'httpx'"))?
	};

	tokio::spawn(async move {
		if let Err(e) = flow::execute_l7(&config.pipeline, &mut container, String::new()).await {
			log(LogLevel::Error, &format!("✗ L7 Flow Logic Failed: {:#}", e));
		}
	});

	// The Driver Loop (The Actor)

	loop {
		tokio::select! {
			// Branch A: Pump Request Body (Stream -> Channel)
			recv_result = stream.recv_data(), if body_tx.is_some() => {
				match recv_result {
					Ok(Some(mut buf)) => {
						let bytes = buf.copy_to_bytes(buf.remaining());
						if let Some(tx) = body_tx.as_ref() {
							if tx.send(Ok(bytes)).await.is_err() {
								body_tx = None;
							}
						}
					}
					Ok(None) => {
						body_tx = None;
					}
					Err(e) => {
						if let Some(tx) = body_tx.as_ref() {
							let _ = tx.send(Err(Error::System(e.to_string()))).await;
						}
						body_tx = None;
					}
				}
			}

			// Branch B: Wait for Response Signal
			res_signal = &mut res_rx => {
				match res_signal {
					Ok(response) => {
						log(LogLevel::Debug, "➜ H3 Driver received response signal.");
						if let Err(e) = stream.send_response(response).await {
							log(LogLevel::Error, &format!("✗ Failed to send H3 headers: {}", e));
						}
						let _ = stream.finish().await;
						break;
					}
					Err(_) => {
						log(LogLevel::Warn, "⚠ L7 Flow finished without Response Signal.");
						break;
					}
				}
			}
		}
	}

	Ok(())
}
