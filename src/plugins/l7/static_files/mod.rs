/* src/plugins/l7/static_files/mod.rs */

pub mod browse;
pub mod inspect;
pub mod range;
pub mod router;

use crate::common::sys::lifecycle::Error;
use crate::engine::interfaces::{
	HttpMiddleware, L7Middleware, MiddlewareOutput, ParamDef, ParamType, Plugin, ResolvedInputs,
};
use crate::layers::l7::{
	container::{Container, PayloadState},
	http::wrapper::VaneBody,
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use futures_util::StreamExt;
use http::HeaderValue;
use http_body::Frame;
use http_body_util::{BodyExt, StreamBody};
use serde_json::Value;
use std::{any::Any, borrow::Cow};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

pub struct StaticPlugin;

impl Plugin for StaticPlugin {
	fn name(&self) -> &'static str {
		"internal.driver.static"
	}

	fn params(&self) -> Vec<ParamDef> {
		vec![
			ParamDef {
				name: Cow::Borrowed("root"),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("uri"),
				required: true,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("index"),
				required: false,
				param_type: ParamType::String,
			},
			ParamDef {
				name: Cow::Borrowed("spa"),
				required: false,
				param_type: ParamType::Boolean,
			},
			ParamDef {
				name: Cow::Borrowed("browse"),
				required: false,
				param_type: ParamType::Boolean,
			},
			ParamDef {
				name: Cow::Borrowed("precompress"),
				required: false,
				param_type: ParamType::Boolean,
			},
			ParamDef {
				name: Cow::Borrowed("symlink"),
				required: false,
				param_type: ParamType::Boolean,
			},
		]
	}

	fn supported_protocols(&self) -> Vec<Cow<'static, str>> {
		vec!["httpx".into()]
	}

	fn as_any(&self) -> &dyn Any {
		self
	}

	fn as_http_middleware(&self) -> Option<&dyn HttpMiddleware> {
		Some(self)
	}

	fn as_l7_middleware(&self) -> Option<&dyn L7Middleware> {
		Some(self)
	}
}

#[async_trait]
impl HttpMiddleware for StaticPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		vec![
			Cow::Borrowed("success"),
			Cow::Borrowed("not_found"),
			Cow::Borrowed("failure"),
		]
	}

	async fn execute(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput> {
		let container = context
			.downcast_mut::<Container>()
			.ok_or_else(|| anyhow::anyhow!("Context is not a Container"))?;

		// 1. Resolve Inputs
		let root = inputs
			.get("root")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow::anyhow!("Missing root"))?;

		// Explicit URI input required (No implicit fallback to req.path)
		let request_path = inputs
			.get("uri")
			.and_then(Value::as_str)
			.ok_or_else(|| anyhow::anyhow!("Missing uri"))?;

		// Remove query string if present in URI input (basic sanitization)
		let uri_path = request_path.split('?').next().unwrap_or("/");

		let index_file = inputs
			.get("index")
			.and_then(Value::as_str)
			.unwrap_or("index.html");
		let spa_mode = inputs.get("spa").and_then(Value::as_bool).unwrap_or(false);
		let browse_enabled = inputs
			.get("browse")
			.and_then(Value::as_bool)
			.unwrap_or(false);
		let precompress = inputs
			.get("precompress")
			.and_then(Value::as_bool)
			.unwrap_or(false);
		let allow_symlinks = inputs
			.get("symlink")
			.and_then(Value::as_bool)
			.unwrap_or(false);

		// 2. Resolve Path
		let mut fs_path = match router::resolve_path(root, uri_path, allow_symlinks) {
			Ok(p) => p,
			Err(e) => {
				// Path traversal attempt or invalid root
				container.kv.insert("res.error".to_string(), e.to_string());
				return Ok(MiddlewareOutput {
					branch: Cow::Borrowed("failure"),
					store: None,
				});
			}
		};

		// 3. File Existence Check & SPA Fallback
		let mut metadata = match tokio::fs::metadata(&fs_path).await {
			Ok(m) => m,
			Err(_) => {
				// Not Found
				if spa_mode {
					// Fallback to root index
					if let Ok(root_path) = router::resolve_path(root, "/", allow_symlinks) {
						let fallback = root_path.join(index_file);
						if let Ok(m) = tokio::fs::metadata(&fallback).await {
							fs_path = fallback;
							m
						} else {
							return Ok(MiddlewareOutput {
								branch: Cow::Borrowed("not_found"),
								store: None,
							});
						}
					} else {
						return Ok(MiddlewareOutput {
							branch: Cow::Borrowed("not_found"),
							store: None,
						});
					}
				} else {
					return Ok(MiddlewareOutput {
						branch: Cow::Borrowed("not_found"),
						store: None,
					});
				}
			}
		};

		// 4. Directory Handling
		if metadata.is_dir() {
			// Try Index
			let index_path = fs_path.join(index_file);
			if let Ok(m) = tokio::fs::metadata(&index_path).await {
				fs_path = index_path;
				metadata = m;
			} else if browse_enabled {
				// Browse Directory
				let html = browse::generate_listing(&fs_path, uri_path).await?;
				container.response_headers.insert(
					http::header::CONTENT_TYPE,
					HeaderValue::from_static("text/html; charset=utf-8"),
				);
				container.response_body = PayloadState::new_buffered(html)?;
				return Ok(MiddlewareOutput {
					branch: Cow::Borrowed("success"),
					store: None,
				});
			} else {
				return Ok(MiddlewareOutput {
					branch: Cow::Borrowed("not_found"),
					store: None,
				});
			}
		}

		// 5. Open File
		let mut file = File::open(&fs_path).await.context("Failed to open file")?;
		let content_type = inspect::determine_mime_type(&fs_path, &mut file).await;
		let mut content_encoding = None;

		// 6. Range Request Detection
		let range_header = container
			.request_headers
			.get(http::header::RANGE)
			.and_then(|h| h.to_str().ok());

		let mut range = None;
		if let Some(rh) = range_header {
			range = range::parse_range_header(rh, metadata.len());
		}

		// 7. Precompression Logic (Only if NO Range)
		if range.is_none() && precompress {
			let accept_encoding = container
				.request_headers
				.get(http::header::ACCEPT_ENCODING)
				.and_then(|h| h.to_str().ok())
				.unwrap_or("");

			// Simple check for gzip
			if accept_encoding.contains("gzip") {
				let gz_path = fs_path.with_extension(format!(
					"{}.gz",
					fs_path.extension().unwrap_or_default().to_string_lossy()
				));
				if let Ok(gz_meta) = tokio::fs::metadata(&gz_path).await {
					if let Ok(gz_file) = File::open(&gz_path).await {
						// Switch to compressed file
						file = gz_file;
						metadata = gz_meta;
						content_encoding = Some("gzip");
						// Content-Type remains that of original file
					}
				}
			}
		}

		// 8. Headers Population

		let headers = &mut container.response_headers;

		let ct_val = HeaderValue::from_str(&content_type)
			.map_err(|e| anyhow!("Invalid content-type generated: {}", e))?;

		headers.insert(http::header::CONTENT_TYPE, ct_val);

		if let Some(enc) = content_encoding {
			headers.insert(
				http::header::CONTENT_ENCODING,
				HeaderValue::from_static(enc),
			);
		}

		let last_modified = inspect::generate_etag(
			metadata.modified().unwrap_or(std::time::SystemTime::now()),
			metadata.len(),
		);

		if let Ok(val) = HeaderValue::from_str(&last_modified) {
			headers.insert(http::header::ETAG, val);
		}

		// 9. Streaming Body Setup

		let mut length = metadata.len();

		if let Some(r) = range {
			// Range Response

			if let Err(_) = file.seek(SeekFrom::Start(r.start)).await {
				container
					.kv
					.insert("res.status".to_string(), "500".to_string());

				return Ok(MiddlewareOutput {
					branch: Cow::Borrowed("failure"),

					store: None,
				});
			}

			length = r.length;

			// 206 Partial Content

			container
				.kv
				.insert("res.status".to_string(), "206".to_string());

			let range_val = format!(
				"bytes {}-{}/{}",
				r.start,
				r.start + r.length - 1,
				metadata.len()
			);

			let cr_val = HeaderValue::from_str(&range_val)
				.map_err(|e| anyhow!("Invalid content-range generated: {}", e))?;

			headers.insert(http::header::CONTENT_RANGE, cr_val);
		} else if range_header.is_some() && range.is_none() {
			// 416 Range Not Satisfiable

			container
				.kv
				.insert("res.status".to_string(), "416".to_string());

			let range_val = format!("bytes */{}", metadata.len());

			let cr_val = HeaderValue::from_str(&range_val)
				.map_err(|e| anyhow!("Invalid content-range generated: {}", e))?;

			headers.insert(http::header::CONTENT_RANGE, cr_val);

			// Return empty body

			container.response_body = PayloadState::Empty;

			return Ok(MiddlewareOutput {
				branch: Cow::Borrowed("success"),

				store: None,
			});
		} else {
			// 200 OK
			headers.insert(
				http::header::ACCEPT_RANGES,
				HeaderValue::from_static("bytes"),
			);
		}

		headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from(length));

		// Create Stream
		// We use `take` to limit reading for range requests
		let stream = ReaderStream::new(file.take(length));

		// Map stream to VaneBody
		let body_stream = stream.map(|result: std::io::Result<bytes::Bytes>| match result {
			Ok(bytes) => Ok(Frame::data(bytes)),
			Err(e) => Err(Error::Io(e.to_string())),
		});

		let boxed_body = BodyExt::boxed(StreamBody::new(body_stream));
		container.response_body = PayloadState::Http(VaneBody::Generic(boxed_body));

		Ok(MiddlewareOutput {
			branch: Cow::Borrowed("success"),
			store: None,
		})
	}
}

#[async_trait]
impl L7Middleware for StaticPlugin {
	fn output(&self) -> Vec<Cow<'static, str>> {
		<Self as HttpMiddleware>::output(self)
	}

	async fn execute_l7(
		&self,
		context: &mut (dyn Any + Send),
		inputs: ResolvedInputs,
	) -> Result<MiddlewareOutput> {
		<Self as HttpMiddleware>::execute(self, context, inputs).await
	}
}
