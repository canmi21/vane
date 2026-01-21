/* src/layers/l4p/context.rs */

use crate::plugins::protocol::{
	quic::parser::QuicInitialData, tls::clienthello::TlsClientHelloData,
};
use crate::resources::kv::KvStore;
use fancy_log::{LogLevel, log};

pub fn inject_common(kv: &mut KvStore, protocol: &str) {
	log(
		LogLevel::Debug,
		&format!("⚙ Injecting L4+ Context for protocol: {protocol}"),
	);

	kv.insert("conn.layer".to_owned(), "l4p".to_owned());
	kv.insert("conn.proto.carrier".to_owned(), protocol.to_owned());
}

pub fn inject_tls_data(kv: &mut KvStore, data: TlsClientHelloData) {
	log(
		LogLevel::Debug,
		&format!(
			"⚙ Parsed ClientHello -> SNI: {:?}, ALPN: {:?}, LegacyVer: {}",
			data.sni, data.alpn, data.legacy_version
		),
	);

	if let Some(sni) = data.sni {
		// Normalization: Lowercase + Character filtering
		let sanitized = sanitize_sni(&sni);
		if sanitized != sni {
			log(
				LogLevel::Debug,
				&format!("⚙ SNI Normalized: '{sni}' -> '{sanitized}'"),
			);
		}
		kv.insert("tls.sni".to_owned(), sanitized);
	} else {
		log(
			LogLevel::Debug,
			"⚙ Warning: SNI field is empty in parsed data.",
		);
	}

	if !data.alpn.is_empty() {
		kv.insert("tls.alpn".to_owned(), data.alpn.join(","));
	}

	kv.insert("tls.version.legacy".to_owned(), data.legacy_version);
	kv.insert("tls.session_id".to_owned(), data.session_id);

	kv.insert("tls.cipher_suites".to_owned(), data.cipher_suites.join(","));
	kv.insert(
		"tls.compression".to_owned(),
		data.compression_methods.join(","),
	);
	kv.insert(
		"tls.supported_versions".to_owned(),
		data.supported_versions.join(","),
	);
	kv.insert(
		"tls.supported_groups".to_owned(),
		data.supported_groups.join(","),
	);
	kv.insert(
		"tls.signature_algorithms".to_owned(),
		data.signature_algorithms.join(","),
	);
	kv.insert(
		"tls.key_share_groups".to_owned(),
		data.key_share_groups.join(","),
	);
	kv.insert(
		"tls.psk_modes".to_owned(),
		data.psk_key_exchange_modes.join(","),
	);

	kv.insert(
		"tls.has_renegotiation_info".to_owned(),
		data.has_renegotiation_info.to_string(),
	);
	kv.insert("tls.has_grease".to_owned(), data.has_grease.to_string());
}

/// Sanitizes SNI string to prevent injection and enforce standard naming.
/// Converts to lowercase and filters out non-standard domain characters.
fn sanitize_sni(sni: &str) -> String {
	sni
		.to_lowercase()
		.chars()
		.filter(|c| c.is_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
		.collect()
}

pub fn inject_quic_data(kv: &mut KvStore, data: QuicInitialData) {
	log(
		LogLevel::Debug,
		&format!(
			"⚙ Parsed QUIC Initial -> DCID: {}, SCID: {}, Ver: {}, SNI: {:?}",
			data.dcid, data.scid, data.version, data.sni_hint
		),
	);

	kv.insert("quic.dcid".to_owned(), data.dcid);
	kv.insert("quic.scid".to_owned(), data.scid);
	kv.insert("quic.version".to_owned(), data.version);

	if let Some(token) = data.token {
		kv.insert("quic.token".to_owned(), token);
	}

	if let Some(sni) = data.sni_hint {
		kv.insert("quic.sni".to_owned(), sanitize_sni(&sni));
	}
}
