/* src/resources/templates/source/http.rs */

use crate::layers::l7::container::Container;
use std::sync::Arc;
use tokio::sync::RwLock;
use varchain::{Resolved, Source, SourceFuture};

pub struct HttpSource {
	pub container: Arc<RwLock<Container>>,
}

impl Source for HttpSource {
	fn get(&self, key: &str) -> SourceFuture<'_, String> {
		let key = key.to_owned();
		let container = self.container.clone();

		Box::pin(async move {
			// 1. Body hijacking (triggers lazy buffering)
			if key == "req.body" {
				let mut c = container.write().await;
				if let Ok(bytes) = c.force_buffer_request().await {
					return Resolved::Found(String::from_utf8_lossy(bytes).into_owned());
				}
				return Resolved::Pass;
			}

			if key == "req.body_hex" {
				let mut c = container.write().await;
				if let Ok(bytes) = c.force_buffer_request().await {
					return Resolved::Found(hex::encode(bytes));
				}
				return Resolved::Pass;
			}

			if key == "res.body" {
				let mut c = container.write().await;
				if let Ok(bytes) = c.force_buffer_response().await {
					return Resolved::Found(String::from_utf8_lossy(bytes).into_owned());
				}
				return Resolved::Pass;
			}

			if key == "res.body_hex" {
				let mut c = container.write().await;
				if let Ok(bytes) = c.force_buffer_response().await {
					return Resolved::Found(hex::encode(bytes));
				}
				return Resolved::Pass;
			}

			// 2. Header access (read-only)
			let c = container.read().await;
			if let Some(header_name) = key.strip_prefix("req.header.") {
				return Resolved::Found(get_header_value(&c.request_headers, header_name));
			}

			if let Some(header_name) = key.strip_prefix("res.header.") {
				return Resolved::Found(get_header_value(&c.response_headers, header_name));
			}

			if key == "req.headers" {
				return Resolved::Found(format!("{:?}", c.request_headers));
			}

			if key == "res.headers" {
				return Resolved::Found(format!("{:?}", c.response_headers));
			}

			Resolved::Pass
		})
	}
}

fn get_header_value(map: &http::HeaderMap, key_name: &str) -> String {
	match map.get(key_name) {
		Some(val) => val.to_str().unwrap_or("").to_owned(),
		None => String::new(),
	}
}
