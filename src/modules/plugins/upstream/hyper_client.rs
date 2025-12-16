/* src/modules/plugins/upstream/hyper_client.rs */

use super::pool::{GLOBAL_INSECURE_CLIENT, GLOBAL_SECURE_CLIENT};
use crate::common::requirements::{Error, Result};
use crate::modules::stack::protocol::application::{
	container::{Container, PayloadState},
	http::wrapper::VaneBody,
};
use bytes::Bytes;
use fancy_log::{LogLevel, log};
use http::{Method, Request, Uri};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use std::str::FromStr;

pub async fn execute_hyper_request(
	container: &mut Container,
	url_str: &str,
	method_str: Option<&str>,
	version_hint: Option<&str>,
	skip_verify: bool,
) -> Result<()> {
	let uri =
		Uri::from_str(url_str).map_err(|e| Error::Configuration(format!("Invalid URL: {}", e)))?;

	let method = if let Some(m) = method_str {
		Method::from_str(m).unwrap_or(Method::GET)
	} else {
		container
			.kv
			.get("req.method")
			.and_then(|m| Method::from_str(m).ok())
			.unwrap_or(Method::GET)
	};

	if let Some(v) = version_hint {
		log(
			LogLevel::Debug,
			&format!(
				"⚙ Upstream Version Hint: {} (Hyper manages this automatically)",
				v
			),
		);
	}

	let req_payload = std::mem::replace(&mut container.request_body, PayloadState::Empty);

	let body: BoxBody<Bytes, Error> = match req_payload {
		PayloadState::Http(vane_body) => vane_body.boxed(),
		PayloadState::Buffered(bytes) => Full::new(bytes).map_err(|e| match e {}).boxed(),
		PayloadState::Empty | PayloadState::Generic => BoxBody::default(),
	};

	let req_builder = Request::builder()
		.method(method)
		.uri(uri)
		.header("User-Agent", concat!("Vane/", env!("CARGO_PKG_VERSION")));

	let req = req_builder
		.body(body)
		.map_err(|e| Error::System(format!("Failed to build upstream request: {}", e)))?;

	let client = if skip_verify {
		&*GLOBAL_INSECURE_CLIENT
	} else {
		&*GLOBAL_SECURE_CLIENT
	};

	log(
		LogLevel::Debug,
		&format!(
			"➜ Fetching Upstream (Hyper): {} {}",
			req.method(),
			req.uri()
		),
	);

	match client.request(req).await {
		Ok(res) => {
			let status = res.status();
			log(
				LogLevel::Debug,
				&format!("✓ Upstream Responded: {}", status),
			);

			container
				.kv
				.insert("res.status".to_string(), status.as_u16().to_string());

			for (k, v) in res.headers() {
				if let Ok(val) = v.to_str() {
					container
						.kv
						.insert(format!("res.header.{}", k), val.to_string());
				}
			}

			let incoming = res.into_body();
			container.response_body = PayloadState::Http(VaneBody::Hyper(incoming));

			Ok(())
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Upstream Request Failed: {}", e),
			);
			Err(Error::System(format!("Upstream error: {}", e)))
		}
	}
}
