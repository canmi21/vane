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
	mut req: Request<Incoming>,
	protocol_id: String,
) -> std::result::Result<Response<BoxBody<Bytes, Error>>, Error> {
	let client_upgrade_handle = if req.headers().contains_key(http::header::UPGRADE)
		|| req.headers().contains_key(http::header::CONNECTION)
	{
		Some(hyper::upgrade::on(&mut req))
	} else {
		None
	};

	let (mut parts, body) = req.into_parts();

	let request_payload = PayloadState::Http(VaneBody::Hyper(body));
	let response_payload = PayloadState::Empty;

	let (res_tx, res_rx) = oneshot::channel::<Response<()>>();

	let mut kv = KvStore::new();
	kv.insert("req.proto".to_string(), protocol_id.clone());
	kv.insert("req.method".to_string(), parts.method.to_string());
	kv.insert("req.path".to_string(), parts.uri.path().to_string());
	kv.insert("req.version".to_string(), format!("{:?}", parts.version));

	if let Some(q) = parts.uri.query() {
		kv.insert("req.query".to_string(), q.to_string());
	}

	if let Some(host) = parts.headers.get("host") {
		if let Ok(h) = host.to_str() {
			kv.insert("req.host".to_string(), h.to_string());
		}
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

	container.client_upgrade = client_upgrade_handle;

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

	match res_rx.await {
		Ok(response_parts) => {
			let (parts, _) = response_parts.into_parts();

			let mut payload = std::mem::replace(&mut container.response_body, PayloadState::Empty);

			if let PayloadState::Http(VaneBody::SwitchingProtocols(upstream_upgrade)) = payload {
				if let Some(client_upgrade) = container.client_upgrade.take() {
					let tunnel_future = Box::pin(async move {
						tokio::task::yield_now().await;

						match tokio::try_join!(client_upgrade, upstream_upgrade) {
							Ok((mut client_io, mut upstream_io)) => {
								let mut client_tokio = TokioIo::new(&mut client_io);
								let mut upstream_tokio = TokioIo::new(&mut upstream_io);

								match tokio::io::copy_bidirectional(&mut client_tokio, &mut upstream_tokio).await {
									Ok((from_client, from_upstream)) => {
										log(
											LogLevel::Debug,
											&format!(
												"✓ Upgrade Tunnel Closed (Client->: {}, <-Upstream: {})",
												from_client, from_upstream
											),
										);
									}
									Err(e) => {
										log(LogLevel::Debug, &format!("⚠ Upgrade Tunnel Error: {}", e));
									}
								}
							}
							Err(e) => {
								log(
									LogLevel::Error,
									&format!("✗ Failed to establish upgrade tunnel: {}", e),
								);
							}
						}
					});

					payload = PayloadState::Http(VaneBody::UpgradeBridge {
						tunnel_task: Some(tunnel_future),
					});
				} else {
					log(
						LogLevel::Error,
						"✗ Response indicates Upgrade, but Client handle is missing!",
					);
					payload = PayloadState::Empty;
				}
			}

			let final_body = convert_payload_to_body(payload);
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

pub(super) fn convert_payload_to_body(payload: PayloadState) -> BoxBody<Bytes, Error> {
	match payload {
		PayloadState::Http(bridge @ VaneBody::UpgradeBridge { .. }) => bridge.boxed(),
		PayloadState::Http(VaneBody::SwitchingProtocols(_)) => BoxBody::default(),
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
