/* src/modules/stack/protocol/application/http/httpx.rs */

use super::wrapper::VaneBody;
use crate::common::requirements::{Error, Result};
use crate::modules::kv::KvStore;
use crate::modules::plugins::model::ConnectionObject;
use crate::modules::stack::protocol::application::{
	container::{Container, PayloadState},
	flow,
	model::APPLICATION_REGISTRY,
};
use fancy_log::{LogLevel, log};

use bytes::Bytes;
use http::{HeaderMap, Request, Response};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use tokio::sync::oneshot;

/// Entry point for Httpx Protocol (L7).
pub async fn handle_connection(conn: ConnectionObject, protocol_id: String) -> Result<()> {
	log(
		LogLevel::Debug,
		&format!("➜ Starting L7 Httpx Engine (Proto: {})...", protocol_id),
	);

	let io = match conn {
		ConnectionObject::Stream(boxed_stream) => TokioIo::new(boxed_stream),
		_ => {
			return Err(Error::System(
				"Httpx engine requires a Stream connection.".into(),
			));
		}
	};

	let service = service_fn(move |req: Request<Incoming>| {
		let proto = protocol_id.clone();
		async move { serve_request(req, proto).await }
	});

	let builder = AutoBuilder::new(hyper_util::rt::TokioExecutor::new());

	if let Err(e) = builder.serve_connection(io, service).await {
		log(
			LogLevel::Error,
			&format!("✗ Httpx Connection Error: {:?}", e),
		);
	}

	Ok(())
}

async fn serve_request(
	req: Request<Incoming>,
	protocol_id: String,
) -> std::result::Result<Response<BoxBody<Bytes, Error>>, Error> {
	// Wrap Hyper Incoming body directly into VaneBody::Hyper
	let (mut parts, body) = req.into_parts();

	// Assign to REQUEST slot
	let request_payload = PayloadState::Http(VaneBody::Hyper(body));
	// Initialize RESPONSE slot
	let response_payload = PayloadState::Empty;

	let (res_tx, res_rx) = oneshot::channel::<Response<()>>();

	// http metadata
	let mut kv = KvStore::new();
	kv.insert("req.proto".to_string(), protocol_id.clone());
	kv.insert("req.method".to_string(), parts.method.to_string());
	kv.insert("req.path".to_string(), parts.uri.path().to_string());
	kv.insert("req.version".to_string(), format!("{:?}", parts.version));

	if let Some(host) = parts.headers.get("host") {
		if let Ok(h) = host.to_str() {
			kv.insert("req.host".to_string(), h.to_string());
		}
	}

	// Pass full HeaderMap to Container Zero-Copy Move
	// We take ownership of parts.headers.
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
		let registry = APPLICATION_REGISTRY.load();
		match registry.get(&protocol_id) {
			Some(c) => c.value().clone(),
			None => {
				log(
					LogLevel::Error,
					&format!("✗ No config for app protocol: {}", protocol_id),
				);
				return Ok(response_error(500, "Configuration Error"));
			}
		}
	};

	if let Err(e) = flow::execute_l7(&config.pipeline, &mut container, "".to_string()).await {
		log(
			LogLevel::Error,
			&format!("✗ L7 Flow Execution Failed: {:#}", e),
		);
		return Ok(response_error(502, "Bad Gateway (Flow Error)"));
	}

	// Wait for the Terminator to signal the response headers
	match res_rx.await {
		Ok(response_parts) => {
			let (parts, _) = response_parts.into_parts();

			// CRITICAL: Retrieve the Response Body from the Container!
			// We extract from response_body slot now.
			let final_body = extract_response_body_from_container(&mut container);

			Ok(Response::from_parts(parts, final_body))
		}
		Err(_) => {
			log(
				LogLevel::Warn,
				"⚠ Flow finished but no response signal received.",
			);
			Ok(response_error(502, "Bad Gateway (No Response Signal)"))
		}
	}
}

/// Helper to extract and convert the Container's RESPONSE payload.
fn extract_response_body_from_container(container: &mut Container) -> BoxBody<Bytes, Error> {
	// Steal the RESPONSE payload
	let payload = std::mem::replace(&mut container.response_body, PayloadState::Empty);

	match payload {
		PayloadState::Http(vane_body) => vane_body.boxed(),
		PayloadState::Buffered(bytes) => Full::new(bytes).map_err(|e| match e {}).boxed(),
		PayloadState::Generic => BoxBody::default(),
		PayloadState::Empty => BoxBody::default(),
	}
}

fn response_error(status: u16, msg: &str) -> Response<BoxBody<Bytes, Error>> {
	let body = Full::new(Bytes::from(msg.to_string()))
		.map_err(|e| match e {})
		.boxed();
	Response::builder().status(status).body(body).unwrap()
}
