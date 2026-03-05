use super::stream::{CgiResponseBody, pump_stdout};
use crate::l7::{
	container::{Container, PayloadState},
	http::wrapper::VaneBody,
};
use anyhow::{Context as AnyhowContext, Result};
use bytes::BytesMut;
use fancy_log::{LogLevel, log};
use http::{HeaderName, HeaderValue};
use http_body_util::combinators::BoxBody;
use std::{borrow::Cow, collections::HashMap, process::Stdio, time::Duration};
use tokio::{
	io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
	process::Command,
	sync::mpsc,
	time::timeout,
};
use vane_engine::engine::interfaces::MiddlewareOutput;
use vane_primitives::common::sys::lifecycle::Error;

pub struct CgiConfig {
	pub command: String,
	pub script: String,
	pub timeout: u64,
	pub method: String,
	pub uri: String,
	pub query: String,
	pub remote_addr: String,
	pub remote_port: String,
	pub server_port: String,
	pub server_name: String,
	pub doc_root: String,
	pub path_info: String,
	pub script_name: String,
}

pub async fn execute(container: &mut Container, config: CgiConfig) -> Result<MiddlewareOutput> {
	let body_timeout_sec = envflag::get::<u64>("CGI_BODY_TIMEOUT_SEC", 30);
	let max_body_size = envflag::get::<usize>("CGI_BODY_MAX_SIZE_BYTE", 10_485_760);

	let body_bytes = container.force_buffer_request().await?.clone();
	let content_length = body_bytes.len().to_string();

	log(
		LogLevel::Debug,
		&format!(
			"⚙ CGI Request: method={}, body_size={} bytes",
			config.method,
			body_bytes.len()
		),
	);

	let mut envs = HashMap::new();
	envs.insert("GATEWAY_INTERFACE".to_owned(), "CGI/1.1".to_owned());
	envs.insert(
		"SERVER_SOFTWARE".to_owned(),
		format!("Vane/{}", env!("CARGO_PKG_VERSION")),
	);
	envs.insert("REDIRECT_STATUS".to_owned(), "200".to_owned());
	envs.insert("SERVER_PROTOCOL".to_owned(), "HTTP/1.1".to_owned());
	envs.insert("SCRIPT_FILENAME".to_owned(), config.script.clone());
	envs.insert("SCRIPT_NAME".to_owned(), config.script_name);
	envs.insert("DOCUMENT_ROOT".to_owned(), config.doc_root.clone());
	envs.insert("PATH_INFO".to_owned(), config.path_info.clone());

	if !config.doc_root.is_empty() && !config.path_info.is_empty() {
		let translated = format!(
			"{}{}",
			config.doc_root.trim_end_matches('/'),
			config.path_info
		);
		envs.insert("PATH_TRANSLATED".to_owned(), translated);
	}

	envs.insert("REQUEST_METHOD".to_owned(), config.method);
	envs.insert("REQUEST_URI".to_owned(), config.uri);
	envs.insert("QUERY_STRING".to_owned(), config.query);
	envs.insert("REMOTE_ADDR".to_owned(), config.remote_addr);
	envs.insert("REMOTE_PORT".to_owned(), config.remote_port);
	envs.insert("SERVER_PORT".to_owned(), config.server_port);
	envs.insert("SERVER_NAME".to_owned(), config.server_name);
	envs.insert("CONTENT_LENGTH".to_owned(), content_length);

	for (k, v) in &container.request_headers {
		let key = k.as_str().to_uppercase().replace('-', "_");
		if let Ok(val) = v.to_str() {
			if key == "CONTENT_TYPE" {
				envs.insert("CONTENT_TYPE".to_owned(), val.to_owned());
			} else if key != "CONTENT_LENGTH" {
				envs.insert(format!("HTTP_{key}"), val.to_owned());
			}
		}
	}

	let mut child = Command::new(&config.command)
		.args(if !config.script.is_empty() {
			vec![&config.script]
		} else {
			vec![]
		})
		.envs(&envs)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.context("Failed to spawn CGI process")
		.map_err(|e| Error::System(e.to_string()))?;

	let mut stdin = child.stdin.take().ok_or_else(|| {
		let _ = child.start_kill();
		Error::System("Failed to open CGI stdin".into())
	})?;
	let mut stdout = child.stdout.take().ok_or_else(|| {
		let _ = child.start_kill();
		Error::System("Failed to open CGI stdout".into())
	})?;
	let stderr = child.stderr.take().ok_or_else(|| {
		let _ = child.start_kill();
		Error::System("Failed to open CGI stderr".into())
	})?;

	tokio::spawn(async move {
		let mut reader = BufReader::new(stderr).lines();
		while let Ok(Some(line)) = reader.next_line().await {
			log(LogLevel::Debug, &format!("⚙ CGI stderr: {line}"));
		}
	});

	// IMPORTANT: Write stdin BEFORE reading stdout to prevent CGI from blocking
	// Many CGI scripts (especially POST handlers) wait for complete stdin before responding
	if !body_bytes.is_empty() {
		if let Err(e) = stdin.write_all(&body_bytes).await {
			log(LogLevel::Warn, &format!("⚠ CGI stdin write failed: {e}"));
			let _ = child.kill().await;
			return Ok(MiddlewareOutput {
				branch: Cow::Borrowed("failure"),
				store: None,
			});
		}
		log(
			LogLevel::Debug,
			&format!("✓ CGI stdin written: {} bytes", body_bytes.len()),
		);
	}
	drop(stdin); // Close stdin to signal EOF to CGI

	let mut header_buffer = BytesMut::new();
	let mut body_start_chunk = BytesMut::new();
	let mut buf_chunk = [0u8; 4096];
	let mut header_parsed = false;

	let read_result = timeout(Duration::from_secs(config.timeout), async {
		loop {
			let n = stdout
				.read(&mut buf_chunk)
				.await
				.map_err(|e| Error::System(e.to_string()))?;
			if n == 0 {
				break;
			}

			header_buffer.extend_from_slice(&buf_chunk[..n]);

			if let Some(idx) = find_double_newline(&header_buffer) {
				let remaining = header_buffer.split_off(idx + 4);
				body_start_chunk = remaining;
				header_buffer.truncate(idx);
				header_parsed = true;
				break;
			}

			if header_buffer.len() > 65536 {
				return Err(Error::System("CGI Header buffer overflow".into()));
			}
		}
		Ok(())
	})
	.await;

	if read_result.is_err() || !header_parsed {
		let _ = child.kill().await;
		log(LogLevel::Warn, "⚠ CGI failed to parse headers or timed out");
		return Ok(MiddlewareOutput {
			branch: Cow::Borrowed("failure"),
			store: None,
		});
	}

	let headers_str = String::from_utf8_lossy(&header_buffer);
	log(
		LogLevel::Debug,
		&format!(
			"⚙ CGI Headers Parsed ({} bytes):\n{}",
			header_buffer.len(),
			headers_str
		),
	);

	for line in headers_str.lines() {
		if let Some((k, v)) = line.split_once(':') {
			let key = k.trim();
			let val = v.trim();

			if key.eq_ignore_ascii_case("Status") {
				// Extract numeric status code from "302 Found" -> "302"
				let status_code = val.split_whitespace().next().unwrap_or("200");
				container
					.kv
					.insert("res.status".to_owned(), status_code.to_owned());
			} else if let (Ok(h_name), Ok(h_val)) = (
				HeaderName::from_bytes(key.as_bytes()),
				HeaderValue::from_str(val),
			) {
				container.response_headers.insert(h_name, h_val);
			}
		}
	}

	let (body_tx, body_rx) = mpsc::channel(16);
	let initial_bytes = body_start_chunk.freeze();

	log(
		LogLevel::Debug,
		&format!(
			"➜ Starting CGI Body Pump (Initial chunk: {} bytes)",
			initial_bytes.len()
		),
	);

	tokio::spawn(async move {
		pump_stdout(
			stdout,
			body_tx,
			initial_bytes,
			max_body_size,
			body_timeout_sec,
		)
		.await;
		let _ = child.wait().await;
	});

	container.response_body = PayloadState::Http(VaneBody::Generic(BoxBody::new(
		CgiResponseBody::new(body_rx),
	)));

	Ok(MiddlewareOutput {
		branch: Cow::Borrowed("success"),
		store: None,
	})
}

fn find_double_newline(data: &[u8]) -> Option<usize> {
	data
		.windows(4)
		.position(|w| w == b"\r\n\r\n")
		.or_else(|| data.windows(2).position(|w| w == b"\n\n"))
}
