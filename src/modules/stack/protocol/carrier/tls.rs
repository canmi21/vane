/* src/modules/stack/protocol/carrier/tls.rs */

use super::{context, flow};
use crate::modules::{
	kv::KvStore, plugins::model::ConnectionObject, stack::protocol::carrier::model::RESOLVER_REGISTRY,
};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use tokio::net::TcpStream;

/// The entry point for handling a TLS upgraded connection.
///
/// This module DOES NOT enforce a handshake. It simply prepares the context
/// and hands over the connection (as a generic Stream) to the L4+ Flow Engine.
///
/// The Flow configuration (`resolver/tls.yaml`) decides whether to:
/// 1. Handshake/Terminate (using `internal.protocol.tls.handshake`)
/// 2. Peek SNI and Proxy (using `internal.protocol.tls.detect` + `internal.transport.proxy`)
/// 3. Pass through to another layer.
pub async fn run(stream: TcpStream, kv: &mut KvStore, parent_path: String) -> Result<()> {
	log(LogLevel::Debug, "➜ [Carrier] Entering TLS L4+ Resolver...");

	// Wrap the raw TCP stream as a "Stream" object.
	// At this point, it is NOT yet decrypted. It is just a byte stream that happens to contain TLS records.
	let conn = ConnectionObject::Stream(Box::new(stream));

	// Inject Common L4+ Context
	// We mark it as "tls" so plugins know what parser helpers to use.
	context::inject_common(kv, "tls");

	// Note: We do NOT inject `tls.sni` or `tls.version` here yet.
	// That is the job of the first plugin in the flow (e.g., detect or handshake).

	// Load Flow Config
	let registry = RESOLVER_REGISTRY.load();
	let config = registry
		.get("tls")
		.ok_or_else(|| anyhow!("No resolver config found for 'tls'"))?;

	// Execute Flow
	if let Err(e) = flow::execute(&config.connection, kv, conn, parent_path).await {
		log(
			LogLevel::Error,
			&format!("✗ TLS Flow execution failed: {}", e),
		);
		return Err(e);
	}

	Ok(())
}
