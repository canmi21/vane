/* src/plugins/protocol/upgrader/decryptor.rs */

use crate::engine::contract::ConnectionObject;
use crate::layers::l7::http::httpx;
use crate::resources::{certs, kv::KvStore};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use std::sync::Arc;
use tokio_rustls::{TlsAcceptor, rustls};

/// Performs TLS termination and hands off the decrypted stream to the L7 engine.
pub async fn terminate_and_handover(
	conn: ConnectionObject,
	kv: &mut KvStore,
	target_protocol: String,
) -> Result<()> {
	// 1. Unwrap the L4+ Stream
	let stream = match conn {
		ConnectionObject::Stream(s) => s,
		_ => {
			return Err(anyhow!(
				"Cannot terminate TLS on non-stream connection object"
			));
		}
	};

	// 2. Determine Certificate Strategy
	let cert_lookup_key = kv
		.get("tls.termination.cert_sni")
		.cloned()
		.or_else(|| kv.get("tls.sni").cloned())
		.unwrap_or_else(|| "default".to_string());

	// 3. Fetch Certificate
	let cert = match certs::arcswap::get_certificate(&cert_lookup_key) {
		Some(c) => c,
		None => {
			if cert_lookup_key != "default" {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Certificate '{}' not found. Falling back to 'default'.",
						cert_lookup_key
					),
				);
				certs::arcswap::get_certificate("default").ok_or_else(|| {
					anyhow!(
						"CRITICAL: Neither '{}' nor 'default' certificate found.",
						cert_lookup_key
					)
				})?
			} else {
				return Err(anyhow!("CRITICAL: Default certificate not found."));
			}
		}
	};

	log(
		LogLevel::Debug,
		&format!(
			"⚙ Terminating TLS using certificate for: '{}'",
			cert_lookup_key
		),
	);

	// 4. Configure ALPN
	let mut server_config = rustls::ServerConfig::builder()
		.with_no_client_auth()
		// FIXED: Use key_clone() helper
		.with_single_cert(cert.certs.clone(), cert.key_clone()?)
		.map_err(|e| anyhow!("Invalid TLS configuration: {}", e))?;

	// Httpx supports both H2 and H1 via ALPN negotiation
	if target_protocol == "httpx" || target_protocol == "h2" || target_protocol == "http/1.1" {
		server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
	}

	let acceptor = TlsAcceptor::from(Arc::new(server_config));

	// 5. Handshake
	match acceptor.accept(stream).await {
		Ok(tls_stream) => {
			log(
				LogLevel::Debug,
				"✓ TLS Handshake successful. Upgrading to L7.",
			);

			// Re-wrap as ConnectionObject::Stream (now decrypted)
			let l7_conn = ConnectionObject::Stream(Box::new(tls_stream));

			// 6. Handover to L7 Adapter
			// Map lifecycle::Error to anyhow::Error
			httpx::handle_connection(l7_conn, target_protocol)
				.await
				.map_err(|e| anyhow!("L7 Engine Error: {}", e))
		}
		Err(e) => {
			log(LogLevel::Error, &format!("✗ TLS Handshake failed: {}", e));
			Err(anyhow::Error::from(e))
		}
	}
}
