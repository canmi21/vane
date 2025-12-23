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

	// Extract Request Body (Memory Move -> Zero-Copy)
	let req_payload = std::mem::replace(&mut container.request_body, PayloadState::Empty);

	let body: BoxBody<Bytes, Error> = match req_payload {
		PayloadState::Http(vane_body) => vane_body.boxed(),
		PayloadState::Buffered(bytes) => Full::new(bytes).map_err(|e| match e {}).boxed(),
		PayloadState::Empty | PayloadState::Generic => BoxBody::default(),
	};

	let mut req_builder = Request::builder().method(method).uri(uri);

	// Propagate Request Headers (Client -(Clone)-> Upstream)
	// Clone the headers to preserve the original request context for logging/debugging.
	if let Some(headers) = req_builder.headers_mut() {
		*headers = container.request_headers.clone();

		// Here MUST remove the client's original 'Host' header.
		// Hyper will automatically set the correct 'Host' header based on the Upstream URI.
		// Keeping the old Host usually causes 404s or SNI errors at the upstream.
		headers.remove(http::header::HOST);
	}

	// Trace Header (Optional, for debugging topology)
	req_builder = req_builder.header("X-Vane-Proxy", env!("CARGO_PKG_VERSION"));

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
		&format!("➜ Fetching Upstream Hyper: {} {}", req.method(), req.uri()),
	);

	match client.request(req).await {
		Ok(mut res) => {
			let status = res.status();
			log(
				LogLevel::Debug,
				&format!("✓ Upstream Responded: {}", status),
			);

			// Update KV for Status
			container
				.kv
				.insert("res.status".to_string(), status.as_u16().to_string());

			// Propagate Response Headers (Upstream -> Client)
			// We take ownership of the upstream headers and place them into the Container.
			// Please Do NOT populate KV loop here.
			// Access is handled on-demand via flow::resolve_inputs_l7 (Smart Hijacking).
			container.response_headers = std::mem::take(res.headers_mut());

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
