/* src/layers/l4p/plain.rs */

use super::{context, flow};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use tokio::net::TcpStream;
use vane_app::l7::http::httpx;
use vane_engine::engine::interfaces::{ConnectionObject, TerminatorResult};
use vane_primitives::kv::KvStore;

#[cfg(feature = "lazycert")]
use vane_primitives::lazycert::CHALLENGE_REGISTRY;

/// Entry point for Plaintext L4+ flows (HTTP).
///
/// Workflow:
/// 1. Peek TCP Stream (Read headers without consuming).
/// 2. Parse Host/Method/Path via `httparse`.
/// 3. Inject into KV (`http.host`, `http.method`, etc.).
/// 4. Execute L4+ Flow (`resolver/http.yaml`).
/// 5. Handle Result: Proxy (L4+) or Upgrade (L7).
pub async fn run(
	stream: TcpStream,
	kv: &mut KvStore,
	parent_path: String,
	protocol: &str,
) -> Result<()> {
	log(
		LogLevel::Debug,
		&format!("➜ Entering Plaintext L4+ Resolver ({protocol})"),
	);

	// 1. Configurable Peek Buffer
	let peek_limit = envflag::get::<usize>("HTTP_PLAIN_HEADER_BUFFER_SIZE", 4096);
	let mut buf = vec![0u8; peek_limit];

	// 2. Peek & Parse
	// We peek so that if the flow decides to Proxy (L4 forwarding), the headers are preserved.
	// If we upgrade to L7, L7 will re-read them, but that's acceptable for the flexibility.
	match stream.peek(&mut buf).await {
		Ok(n) if n > 0 => {
			let data = &buf[..n];
			// Attempt to parse HTTP headers to extract routing info
			let mut headers = [httparse::EMPTY_HEADER; 32];
			let mut req = httparse::Request::new(&mut headers);

			match req.parse(data) {
				Ok(httparse::Status::Complete(_) | httparse::Status::Partial) => {
					// Extract Method
					if let Some(m) = req.method {
						kv.insert("http.method".to_owned(), m.to_owned());
					}
					// Extract Path
					if let Some(p) = req.path {
						kv.insert("http.path".to_owned(), p.to_owned());
					}
					// Extract Host Header
					for h in req.headers {
						if h.name.eq_ignore_ascii_case("Host") {
							let host_val = String::from_utf8_lossy(h.value);
							kv.insert("http.host".to_owned(), host_val.to_string());
							break;
						}
					}
					log(
						LogLevel::Debug,
						&format!(
							"⚙ L4+ HTTP Context: Host={:?}, Method={:?}",
							kv.get("http.host"),
							kv.get("http.method")
						),
					);
				}
				Err(_) => {
					log(
						LogLevel::Debug,
						"⚙ Failed to parse HTTP headers in L4+ peek (Non-HTTP traffic?)",
					);
				}
			}
		}
		Ok(_) => { /* Empty stream */ }
		Err(e) => {
			log(LogLevel::Warn, &format!("⚠ Failed to peek TCP stream: {e}"));
		}
	}

	// Check for ACME HTTP-01 challenge BEFORE normal flow
	#[cfg(feature = "lazycert")]
	if let Some(path) = kv.get("http.path")
		&& let Some(token) = path.strip_prefix("/.well-known/acme-challenge/")
		&& !token.is_empty()
	{
		return handle_acme_challenge(stream, token).await;
	}

	let conn = ConnectionObject::Stream(Box::new(stream));
	context::inject_common(kv, protocol);

	// 3. Load & Execute L4+ Flow
	let config_manager = vane_engine::config::get();
	let config = config_manager
		.resolvers
		.get(protocol)
		.ok_or_else(|| anyhow!("No resolver config found for '{protocol}'"))?;

	let execution_result = flow::execute(
		&config.connection,
		kv,
		conn,
		parent_path,
		ahash::AHashMap::new(),
	)
	.await;

	// 4. Handle Outcome
	match execution_result {
		Ok(TerminatorResult::Finished) => {
			// Connection handled at L4+ layer (e.g., L4 Proxy, Deny, etc.)
			Ok(())
		}
		Ok(TerminatorResult::Upgrade {
			protocol: target_proto,
			conn,
			parent_path: _,
		}) => {
			// 5. Upgrade to L7 (httpx)
			// Valid targets: httpx, h1, h2, http/1.1
			if matches!(target_proto.as_str(), "httpx" | "http/1.1" | "h1" | "h2") {
				handle_plain_handover(conn, target_proto).await
			} else {
				Err(anyhow!(
					"Unsupported L7 upgrade protocol from Plaintext: {target_proto}"
				))
			}
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Plain Flow execution failed: {e:#}"),
			);
			Err(e)
		}
	}
}

/// Hands over the TCP stream to the L7 Engine.
async fn handle_plain_handover(conn: ConnectionObject, target_protocol: String) -> Result<()> {
	log(
		LogLevel::Debug,
		&format!("➜ Handing over to L7 Engine ({target_protocol})..."),
	);

	httpx::handle_connection(conn, target_protocol)
		.await
		.map_err(|e| anyhow!("L7 Engine Error: {e}"))
}

/// Handle ACME HTTP-01 challenge response
#[cfg(feature = "lazycert")]
async fn handle_acme_challenge(mut stream: TcpStream, token: &str) -> Result<()> {
	use tokio::io::AsyncWriteExt;

	let response = if let Some(entry) = CHALLENGE_REGISTRY.get(token) {
		let body = &entry.key_authorization;
		format!(
			"HTTP/1.1 200 OK\r\n\
                 Content-Type: text/plain\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
			body.len(),
			body
		)
	} else {
		let body = "Challenge not found";
		format!(
			"HTTP/1.1 404 Not Found\r\n\
                 Content-Type: text/plain\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
			body.len(),
			body
		)
	};

	stream.write_all(response.as_bytes()).await?;
	stream.shutdown().await?;

	log(
		LogLevel::Debug,
		&format!("ACME HTTP-01 challenge response for token: {token}"),
	);

	Ok(())
}
