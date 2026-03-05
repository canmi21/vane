/* src/app/src/l7/http/httpx.rs */

use super::wrapper::VaneBody;
use crate::l7::{
	container::{Container, PayloadState},
	flow,
};
use bytes::Bytes;
use fancy_log::{LogLevel, log};
use http::{HeaderMap, Request, Response};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use tokio::sync::oneshot;
use vane_engine::engine::interfaces::ConnectionObject;
use vane_primitives::common::sys::lifecycle::{Error, Result};
use vane_primitives::kv::KvStore;

/// Entry point for Httpx Protocol (L7).
pub async fn handle_connection(conn: ConnectionObject, protocol_id: String) -> Result<()> {
	log(LogLevel::Debug, &format!("➜ Starting L7 Httpx Engine (Proto: {protocol_id})..."));

	let io = match conn {
		ConnectionObject::Stream(boxed_stream) => TokioIo::new(boxed_stream),
		_ => {
			return Err(Error::System("Httpx engine requires a Stream connection.".into()));
		}
	};

	let service = service_fn(move |req: Request<Incoming>| {
		let proto = protocol_id.clone();
		async move { serve_request(req, proto).await }
	});

	let builder = AutoBuilder::new(hyper_util::rt::TokioExecutor::new());

	// Please Use serve_connection_with_upgrades for WebSocket support
	//
	// This is the correct solution for handling HTTP/1.1 WebSocket upgrades.
	// Earlier implementations attempted various workarounds including:
	// - Manual upgrade handling with oneshot channels
	// - Spawning tunnel tasks with type coercion hacks
	// - Trying to work around !Sync types (hyper::upgrade::Upgraded)
	//
	// All of these approaches either failed with "upgrade expected but low level API in use"
	// or triggered rustc ICE due to broken MIR Unsize coercion when attempting to coerce
	// !Sync async blocks into Sync trait objects.
	//
	// The proper solution is simply to use hyper-util's serve_connection_with_upgrades API,
	// which handles the upgrade protocol correctly at the connection level.
	//
	// Related rustc ICE issue: https://github.com/rust-lang/rust/issues/150378
	if let Err(e) = builder.serve_connection_with_upgrades(io, service).await {
		log(LogLevel::Error, &format!("✗ Httpx Connection Error: {e:?}"));
	}

	Ok(())
}

async fn serve_request(
	mut req: Request<Incoming>,
	protocol_id: String,
) -> std::result::Result<Response<BoxBody<Bytes, Error>>, Error> {
	// Detect HTTP/1.1 WebSocket Upgrade request before destructuring
	// Only H1.1 supports 101 Switching Protocols (H2/H3 use different mechanisms)
	let version = req.version();
	let is_h1_websocket_upgrade = (version == http::Version::HTTP_11
		|| version == http::Version::HTTP_10)
		&& req
			.headers()
			.get("upgrade")
			.and_then(|v| v.to_str().ok())
			.map(|v| v.eq_ignore_ascii_case("websocket"))
			.unwrap_or(false)
		&& req
			.headers()
			.get("connection")
			.and_then(|v| v.to_str().ok())
			.map(|v| v.to_lowercase().contains("upgrade"))
			.unwrap_or(false);

	// Capture client upgrade handle before destructuring request
	let client_upgrade =
		if is_h1_websocket_upgrade { Some(hyper::upgrade::on(&mut req)) } else { None };

	// Now safe to destructure request
	let (mut parts, body) = req.into_parts();

	// Assign to REQUEST slot
	let request_payload = PayloadState::Http(VaneBody::Hyper(body));
	// Initialize RESPONSE slot
	let response_payload = PayloadState::Empty;

	let (res_tx, res_rx) = oneshot::channel::<Response<()>>();

	// http metadata
	let mut kv = KvStore::new();
	kv.insert("req.proto".to_owned(), protocol_id.clone());
	kv.insert("req.method".to_owned(), parts.method.to_string());
	kv.insert("req.path".to_owned(), parts.uri.path().to_owned());
	kv.insert("req.version".to_owned(), format!("{:?}", parts.version));

	// Inject Query String
	if let Some(q) = parts.uri.query() {
		kv.insert("req.query".to_owned(), q.to_owned());
	}

	if let Some(host) = parts.headers.get("host")
		&& let Ok(h) = host.to_str()
	{
		kv.insert("req.host".to_owned(), h.to_owned());
	}

	// Pass full HeaderMap to Container Zero-Copy Move
	// We take ownership of parts.headers.
	let request_headers = std::mem::take(&mut parts.headers);
	let response_headers = HeaderMap::new();

	// Create container with HTTP protocol data (for WebSocket support)
	let mut container = Container::new_with_http(
		kv,
		request_headers,
		request_payload,
		response_headers,
		response_payload,
		Some(res_tx),
	);

	// Inject client upgrade handle if present
	if let Some(upgrade) = client_upgrade
		&& let Some(http_data) = container.http_data_mut()
	{
		http_data.client_upgrade = Some(upgrade);
	}

	let config = {
		let config_manager = vane_engine::config::get();
		if let Some(c) = config_manager.applications.get(&protocol_id) {
			c.clone()
		} else {
			log(LogLevel::Error, &format!("✗ No config for app protocol: {protocol_id}"));
			return response_error(500, "Configuration Error");
		}
	};

	if let Err(e) = flow::execute_l7(&config.pipeline, &mut container, "".to_owned()).await {
		log(LogLevel::Error, &format!("✗ L7 Flow Execution Failed: {e:#}"));
		return response_error(502, "Bad Gateway (Flow Error)");
	}

	// Wait for the Terminator to signal the response headers
	if let Ok(response_parts) = res_rx.await {
		let (parts, _) = response_parts.into_parts();

		// Retrieve the Response Body from the Container!
		// We extract from response_body slot now.
		let final_body = extract_response_body_from_container(&mut container);

		Ok(Response::from_parts(parts, final_body))
	} else {
		log(LogLevel::Warn, "⚠ Flow finished but no response signal received.");
		Ok(response_error(502, "Bad Gateway (No Response Signal)")?)
	}
}

/// Helper to extract and convert the Container's RESPONSE payload.
// Changed visibility to pub(super) so h3.rs can access it
pub(super) fn extract_response_body_from_container(
	container: &mut Container,
) -> BoxBody<Bytes, Error> {
	// Steal the RESPONSE payload using mem::replace to avoid move errors with Drop
	let payload = std::mem::replace(&mut container.response_body, PayloadState::Empty);

	match payload {
		PayloadState::Http(vane_body) => vane_body.boxed(),
		PayloadState::Buffered(bytes, _guard) => Full::new(bytes).map_err(|e| match e {}).boxed(),
		PayloadState::Generic | PayloadState::Empty => BoxBody::default(),
	}
}

fn response_error(status: u16, msg: &str) -> Result<Response<BoxBody<Bytes, Error>>> {
	let body = Full::new(Bytes::from(msg.to_owned())).map_err(|e| match e {}).boxed();
	Response::builder()
		.status(status)
		.body(body)
		.map_err(|e| Error::System(format!("Failed to build error response: {e}")))
}
