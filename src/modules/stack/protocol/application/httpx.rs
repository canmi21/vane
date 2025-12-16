/* src/modules/stack/protocol/application/httpx.rs */

use super::container::{Container, PayloadState};
use super::flow;
use super::model::APPLICATION_REGISTRY;
use crate::common::requirements::{Error, Result};
use crate::modules::kv::KvStore;
use crate::modules::plugins::model::ConnectionObject;
use fancy_log::{LogLevel, log};

use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto::Builder as AutoBuilder;
// Removed unused TcpStream

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
) -> std::result::Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
	let (parts, body) = req.into_parts();

	let boxed_body = body.map_err(|e| e).boxed();
	let payload = PayloadState::Http(boxed_body);

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

	let mut container = Container::new(kv, payload);

	let registry = APPLICATION_REGISTRY.load();
	let config = match registry.get(&protocol_id) {
		Some(c) => c,
		None => {
			log(
				LogLevel::Error,
				&format!("✗ No config for app protocol: {}", protocol_id),
			);
			return Ok(response_error(500, "Configuration Error"));
		}
	};

	match flow::execute_l7(&config.pipeline, &mut container, "".to_string()).await {
		Ok(_) => {
			let msg = format!("Vane L7 Handled: {} {}", parts.method, parts.uri.path());
			Ok(response_ok(msg))
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ L7 Flow Execution Failed: {:#}", e),
			);
			Ok(response_error(502, "Bad Gateway (Flow Error)"))
		}
	}
}

fn response_ok(msg: String) -> Response<BoxBody<Bytes, hyper::Error>> {
	let body = Full::new(Bytes::from(msg)).map_err(|e| match e {}).boxed();
	Response::builder().status(200).body(body).unwrap()
}

fn response_error(status: u16, msg: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
	let body = Full::new(Bytes::from(msg.to_string()))
		.map_err(|e| match e {})
		.boxed();
	Response::builder().status(status).body(body).unwrap()
}
