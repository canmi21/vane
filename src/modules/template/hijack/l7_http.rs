/* src/modules/template/hijack/l7_http.rs */

use super::Hijacker;
use anyhow::Result;
use async_trait::async_trait;

use crate::modules::stack::protocol::application::container::Container;

/// HTTP-specific hijacker for L7 layer
pub struct HttpHijacker<'a> {
	pub container: &'a mut Container,
}

#[async_trait]
impl<'a> Hijacker for HttpHijacker<'a> {
	fn can_handle(&self, key: &str) -> bool {
		matches!(
			key,
			"req.body" | "req.body_hex" | "res.body" | "res.body_hex" | "req.headers" | "res.headers"
		) || key.starts_with("req.header.")
			|| key.starts_with("res.header.")
	}

	async fn resolve(&mut self, key: &str) -> Result<String> {
		// 1. Body hijacking (triggers lazy buffering)
		if key == "req.body" {
			let bytes = self.container.force_buffer_request().await?;
			return Ok(String::from_utf8_lossy(bytes).to_string());
		}

		if key == "req.body_hex" {
			let bytes = self.container.force_buffer_request().await?;
			return Ok(hex::encode(bytes));
		}

		if key == "res.body" {
			let bytes = self.container.force_buffer_response().await?;
			return Ok(String::from_utf8_lossy(bytes).to_string());
		}

		if key == "res.body_hex" {
			let bytes = self.container.force_buffer_response().await?;
			return Ok(hex::encode(bytes));
		}

		// 2. Header access
		if let Some(header_name) = key.strip_prefix("req.header.") {
			return Ok(get_header_value(
				&self.container.request_headers,
				header_name,
			));
		}

		if let Some(header_name) = key.strip_prefix("res.header.") {
			return Ok(get_header_value(
				&self.container.response_headers,
				header_name,
			));
		}

		if key == "req.headers" {
			return Ok(format!("{:?}", self.container.request_headers));
		}

		if key == "res.headers" {
			return Ok(format!("{:?}", self.container.response_headers));
		}

		anyhow::bail!("Unsupported HTTP hijack key: {}", key)
	}
}

fn get_header_value(map: &http::HeaderMap, key_name: &str) -> String {
	match map.get(key_name) {
		Some(val) => val.to_str().unwrap_or("").to_string(),
		None => String::new(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::kv::KvStore;
	use crate::modules::stack::protocol::application::container::PayloadState;

	/// Tests can_handle returns true for known hijack keys.
	#[test]
	fn test_can_handle() {
		let mut container = Container::new(
			KvStore::new(),
			http::HeaderMap::new(),
			PayloadState::Empty,
			http::HeaderMap::new(),
			PayloadState::Empty,
			None,
		);
		let hijacker = HttpHijacker {
			container: &mut container,
		};

		assert!(hijacker.can_handle("req.body"));
		assert!(hijacker.can_handle("req.body_hex"));
		assert!(hijacker.can_handle("res.body"));
		assert!(hijacker.can_handle("res.body_hex"));
		assert!(hijacker.can_handle("req.header.host"));
		assert!(hijacker.can_handle("res.header.content-type"));
		assert!(hijacker.can_handle("req.headers"));
		assert!(hijacker.can_handle("res.headers"));

		assert!(!hijacker.can_handle("conn.ip"));
		assert!(!hijacker.can_handle("random.key"));
	}

	/// Tests get_header_value returns empty string when header not found.
	#[test]
	fn test_get_header_value_not_found() {
		let map = http::HeaderMap::new();
		let result = get_header_value(&map, "host");
		assert_eq!(result, "");
	}
}
