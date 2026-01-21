/* src/plugins/l7/upstream/hyper_client.rs */

use super::pool::{GLOBAL_INSECURE_CLIENT, GLOBAL_SECURE_CLIENT};
use crate::common::sys::lifecycle::{Error, Result};
use crate::layers::l7::{
	container::{Container, PayloadState},
	http::wrapper::VaneBody,
};
use bytes::Bytes;
use fancy_log::{LogLevel, log};
use http::{Method, Request, StatusCode, Uri};
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
		Uri::from_str(url_str).map_err(|e| Error::Configuration(format!("Invalid URL: {e}")))?;

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
			&format!("⚙ Upstream Version Hint: {v} (Hyper Auto Selected)"),
		);
	}

	// Extract Request Body (Memory Move -> Zero-Copy)
	let req_payload = std::mem::replace(&mut container.request_body, PayloadState::Empty);

	let body: BoxBody<Bytes, Error> = match req_payload {
		PayloadState::Http(vane_body) => vane_body.boxed(),
		PayloadState::Buffered(bytes, _guard) => Full::new(bytes).map_err(|e| match e {}).boxed(),
		PayloadState::Empty | PayloadState::Generic => BoxBody::default(),
	};

	let mut req_builder = Request::builder().method(method).uri(uri);

	// Propagate Request Headers (Client -(Clone)-> Upstream)
	if let Some(headers) = req_builder.headers_mut() {
		*headers = container.request_headers.clone();
		headers.remove(http::header::HOST);
	}

	// Trace Header (Optional, for debugging topology)
	req_builder = req_builder.header("X-Vane-Proxy", env!("CARGO_PKG_VERSION"));

	let req = req_builder
		.body(body)
		.map_err(|e| Error::System(format!("Failed to build upstream request: {e}")))?;

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
				&format!("✓ Upstream Responded: {status}"),
			);

			// Update KV for Status
			container
				.kv
				.insert("res.status".to_owned(), status.as_u16().to_string());

			// Propagate Response Headers (Upstream -> Client)
			container.response_headers = std::mem::take(res.headers_mut());

			let incoming = res.into_body();
			container.response_body = PayloadState::Http(VaneBody::Hyper(incoming));

			Ok(())
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Upstream Request Failed: {e}"),
			);
			Err(Error::System(format!("Upstream error: {e}")))
		}
	}
}

/// Execute HTTP/1.1 WebSocket upgrade request to upstream.
/// This function forces HTTP/1.1 and handles 101 Switching Protocols response.
pub async fn execute_h1_websocket_request(
	container: &mut Container,
	url_str: &str,
	method_str: Option<&str>,
	skip_verify: bool,
) -> Result<()> {
	let uri =
		Uri::from_str(url_str).map_err(|e| Error::Configuration(format!("Invalid URL: {e}")))?;

	let method = if let Some(m) = method_str {
		Method::from_str(m).unwrap_or(Method::GET)
	} else {
		Method::GET // WebSocket upgrade typically uses GET
	};

	// Extract Request Body (usually empty for WebSocket upgrade)
	let req_payload = std::mem::replace(&mut container.request_body, PayloadState::Empty);

	let body: BoxBody<Bytes, Error> = match req_payload {
		PayloadState::Http(vane_body) => vane_body.boxed(),
		PayloadState::Buffered(bytes, _guard) => Full::new(bytes).map_err(|e| match e {}).boxed(),
		PayloadState::Empty | PayloadState::Generic => BoxBody::default(),
	};

	let mut req_builder = Request::builder()
		.method(method)
		.uri(uri)
		.version(http::Version::HTTP_11); // Force H1.1

	// Propagate Request Headers (including Upgrade, Sec-WebSocket-* headers)
	if let Some(headers) = req_builder.headers_mut() {
		*headers = container.request_headers.clone();
		headers.remove(http::header::HOST); // Hyper will set correct Host
	}

	req_builder = req_builder.header("X-Vane-Proxy", env!("CARGO_PKG_VERSION"));

	let req = req_builder
		.body(body)
		.map_err(|e| Error::System(format!("Failed to build WebSocket request: {e}")))?;

	let client = if skip_verify {
		&*GLOBAL_INSECURE_CLIENT
	} else {
		&*GLOBAL_SECURE_CLIENT
	};

	log(
		LogLevel::Debug,
		&format!(
			"➜ WebSocket Upgrade Request: {} {}",
			req.method(),
			req.uri()
		),
	);

	match client.request(req).await {
		Ok(mut res) => {
			let status = res.status();
			log(LogLevel::Debug, &format!("✓ Backend Responded: {status}"));

			container
				.kv
				.insert("res.status".to_owned(), status.as_u16().to_string());

			// Propagate Response Headers
			container.response_headers = std::mem::take(res.headers_mut());

			if status == StatusCode::SWITCHING_PROTOCOLS {
				// Backend agreed to upgrade! Capture upstream upgrade handle
				log(LogLevel::Debug, "✓ Backend accepted WebSocket upgrade");

				let upstream_upgrade = hyper::upgrade::on(res);
				if let Some(http_data) = container.http_data_mut() {
					http_data.upstream_upgrade = Some(upstream_upgrade);
				}

				// 101 response has no body
				container.response_body = PayloadState::Empty;
			} else {
				// Backend rejected upgrade, treat as normal HTTP response
				log(
					LogLevel::Debug,
					&format!("⚠ Backend rejected WebSocket upgrade (status: {status})"),
				);

				let incoming = res.into_body();
				container.response_body = PayloadState::Http(VaneBody::Hyper(incoming));
			}

			Ok(())
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ WebSocket Upgrade Request Failed: {e}"),
			);
			Err(Error::System(format!("WebSocket request error: {e}")))
		}
	}
}
