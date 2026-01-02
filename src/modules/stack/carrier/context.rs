/* src/modules/stack/carrier/context.rs */

use crate::modules::{
	kv::KvStore,
	plugins::protocol::{quic::parser::QuicInitialData, tls::clienthello::TlsClientHelloData},
};
use fancy_log::{LogLevel, log};

pub fn inject_common(kv: &mut KvStore, protocol: &str) {
	log(
		LogLevel::Debug,
		&format!("⚙ Injecting L4+ Context for protocol: {}", protocol),
	);

	kv.insert("conn.layer".to_string(), "l4p".to_string());
	kv.insert("conn.proto.carrier".to_string(), protocol.to_string());
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
		kv.insert("tls.sni".to_string(), sni);
	} else {
		log(
			LogLevel::Debug,
			"⚙ Warning: SNI field is empty in parsed data.",
		);
	}

	if !data.alpn.is_empty() {
		kv.insert("tls.alpn".to_string(), data.alpn.join(","));
	}

	kv.insert("tls.version.legacy".to_string(), data.legacy_version);
	kv.insert("tls.session_id".to_string(), data.session_id);

	kv.insert(
		"tls.cipher_suites".to_string(),
		data.cipher_suites.join(","),
	);
	kv.insert(
		"tls.compression".to_string(),
		data.compression_methods.join(","),
	);
	kv.insert(
		"tls.supported_versions".to_string(),
		data.supported_versions.join(","),
	);
	kv.insert(
		"tls.supported_groups".to_string(),
		data.supported_groups.join(","),
	);
	kv.insert(
		"tls.signature_algorithms".to_string(),
		data.signature_algorithms.join(","),
	);
	kv.insert(
		"tls.key_share_groups".to_string(),
		data.key_share_groups.join(","),
	);
	kv.insert(
		"tls.psk_modes".to_string(),
		data.psk_key_exchange_modes.join(","),
	);

	kv.insert(
		"tls.has_renegotiation_info".to_string(),
		data.has_renegotiation_info.to_string(),
	);
	kv.insert("tls.has_grease".to_string(), data.has_grease.to_string());
}

pub fn inject_quic_data(kv: &mut KvStore, data: QuicInitialData) {
	log(
		LogLevel::Debug,
		&format!(
			"⚙ Parsed QUIC Initial -> DCID: {}, SCID: {}, Ver: {}, SNI: {:?}",
			data.dcid, data.scid, data.version, data.sni_hint
		),
	);

	kv.insert("quic.dcid".to_string(), data.dcid);
	kv.insert("quic.scid".to_string(), data.scid);
	kv.insert("quic.version".to_string(), data.version);

	if let Some(token) = data.token {
		kv.insert("quic.token".to_string(), token);
	}

	if let Some(sni) = data.sni_hint {
		kv.insert("quic.sni".to_string(), sni);
	}
}
