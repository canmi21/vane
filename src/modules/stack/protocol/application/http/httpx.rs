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
use http::{Request, Response};
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
	let (parts, body) = req.into_parts();

	// WRAPPER INTEGRATION:
	// Wrap Hyper Incoming body directly into VaneBody::Hyper
	let payload = PayloadState::Http(VaneBody::Hyper(body));

	// SETUP RESPONSE CHANNEL
	// The Flow Terminator will send the response (Headers/Status) to this channel.
	// The Body will be left in the Container.
	let (res_tx, res_rx) = oneshot::channel::<Response<()>>();

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

	// Pass response_tx to Container
	let mut container = Container::new(kv, payload, Some(res_tx));

	let config = {
		let registry = APPLICATION_REGISTRY.load();
		match registry.get(&protocol_id) {
			Some(c) => c.value().clone(), // Clone ARC
			None => {
				log(
					LogLevel::Error,
					&format!("✗ No config for app protocol: {}", protocol_id),
				);
				return Ok(response_error(500, "Configuration Error"));
			}
		}
	};

	// Execute Flow
	// This will run Middlewares -> FetchUpstream (fills payload) -> Terminator (sends signal)
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
			// The container holds the payload which might be:
			// 1. The original Upstream Response Stream (VaneBody::Hyper/H3)
			// 2. A Buffered Response (if WAF ran)
			// 3. Generated Content (if Terminator set it)
			let final_body = extract_body_from_container(&mut container);

			Ok(Response::from_parts(parts, final_body))
		}
		Err(_) => {
			// Terminator didn't run or failed to send response
			log(
				LogLevel::Warn,
				"⚠ Flow finished but no response signal received.",
			);
			Ok(response_error(502, "Bad Gateway (No Response Signal)"))
		}
	}
}

/// Helper to extract and convert the Container's payload into a Hyper-compatible BoxBody.
fn extract_body_from_container(container: &mut Container) -> BoxBody<Bytes, Error> {
	// Steal the payload
	let payload = std::mem::replace(&mut container.payload, PayloadState::Empty);

	match payload {
		PayloadState::Http(vane_body) => {
			// VaneBody implements Body<Data=Bytes, Error=Error>
			// We just need to box it to erase the specific VaneBody type
			vane_body.boxed()
		}
		PayloadState::Buffered(bytes) => {
			// Convert Bytes to a Body
			Full::new(bytes).map_err(|e| match e {}).boxed()
		}
		PayloadState::Generic => {
			// Treat generic stream as empty for HTTP
			BoxBody::default()
		}
		PayloadState::Empty => BoxBody::default(),
	}
}

fn response_error(status: u16, msg: &str) -> Response<BoxBody<Bytes, Error>> {
	let body = Full::new(Bytes::from(msg.to_string()))
		.map_err(|e| match e {}) // Infallible -> Error
		.boxed();
	Response::builder().status(status).body(body).unwrap()
}
