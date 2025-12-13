/* src/modules/stack/protocol/carrier/context.rs */

use crate::modules::kv::KvStore;
use fancy_log::{LogLevel, log};

/// Injects standard L4+ context variables into the KV Store.
pub fn inject_common(kv: &mut KvStore, protocol: &str) {
	log(
		LogLevel::Debug,
		&format!("⚙ Injecting L4+ Context for protocol: {}", protocol),
	);

	// Core Layer Info
	kv.insert("conn.layer".to_string(), "l4plus".to_string());
	kv.insert("conn.proto".to_string(), protocol.to_string());

	// We can add more generic L4+ stats here later (e.g., handshake duration)
}

/// Injects TLS specific context.
pub fn inject_tls(kv: &mut KvStore, sni: Option<&str>, version: &str) {
	if let Some(domain) = sni {
		kv.insert("tls.sni".to_string(), domain.to_string());
	}
	kv.insert("tls.version".to_string(), version.to_string());
}

/// Injects QUIC specific context.
pub fn inject_quic(kv: &mut KvStore, sni: Option<&str>) {
	if let Some(domain) = sni {
		kv.insert("quic.sni".to_string(), domain.to_string());
	}
	kv.insert("quic.alpn".to_string(), "h3".to_string()); // Default or extracted
}
