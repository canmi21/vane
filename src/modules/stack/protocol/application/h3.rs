/* src/modules/stack/protocol/application/h3.rs */

use super::container::{Container, PayloadState};
use super::flow;
use super::model::APPLICATION_REGISTRY;
use crate::common::requirements::{Error, Result};
use crate::modules::kv::KvStore;
use fancy_log::{LogLevel, log};

use bytes::Buf;
use h3::server::RequestStream;
use h3_quinn::quinn::Connection;
use http::{Request, Response};

pub async fn handle_connection(quic_conn: Connection) -> Result<()> {
	log(LogLevel::Debug, "➜ Starting L7 H3 Engine...");

	let h3_quinn_conn = h3_quinn::Connection::new(quic_conn);

	// Explicitly specify Buffer type as bytes::Bytes
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
					// Use resolve_request() to get (Request, Stream)
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

	let payload = PayloadState::Empty;

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

	let mut container = Container::new(kv, payload);

	let registry = APPLICATION_REGISTRY.load();
	let config = registry
		.get("h3")
		.or_else(|| registry.get("httpx"))
		.ok_or_else(|| anyhow::anyhow!("No application config found for 'h3' or 'httpx'"))?;

	match flow::execute_l7(&config.pipeline, &mut container, String::new()).await {
		Ok(_) => {
			let response = Response::builder().status(200).body(()).unwrap();
			match stream.send_response(response).await {
				Ok(_) => {
					let _ = stream.finish().await;
				}
				Err(e) => log(
					LogLevel::Error,
					&format!("✗ Failed to send H3 response: {}", e),
				),
			}
		}
		Err(e) => {
			log(LogLevel::Error, &format!("✗ L7 Flow Failed: {:#}", e));
			let resp = Response::builder().status(502).body(()).unwrap();
			let _ = stream.send_response(resp).await;
			let _ = stream.finish().await;
		}
	}

	Ok(())
}
