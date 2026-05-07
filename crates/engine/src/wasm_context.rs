//! Pack `inspects`-declared field paths into the WASM dispatch
//! `context` channel.
//!
//! Spec: `wasm-abi.md` § _Context exposure_ and § _Path grammar_;
//! `architecture/spec/crates/engine-wasm.md` § _Plugin metadata drives compilation_.
//! Capability semantics — the host packs only declared paths; reading
//! any other field from a plugin is impossible because the data is not
//! delivered.
//!
//! Path validation is the load-time responsibility of
//! `vane_wasm::inspects::validate_inspects_path`. By the time a path
//! reaches `pack_context` here it is guaranteed to be in the
//! documented grammar; the dispatch path therefore only has to decide
//! between (a) connection-level paths, which it packs, and (b)
//! request / response-level paths, which it currently defers.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use vane_core::{ConnContext, ContextEntry, ContextValue, TlsInfo, TlsVersion, Transport};

/// Pack the inspects-declared paths into a `Vec<ContextEntry>`. Any
/// declared connection-level path produces an entry — absent sources
/// (`conn.tls.sni` on a non-TLS listener, etc.) pack as the spec's
/// empty value, not as omission, so plugins see a stable schema. A
/// declared path the host doesn't currently pack (request / response
/// scope) is skipped after a warn-once per `(module_id, path)`.
#[must_use]
pub(crate) fn pack_context(
	inspects: &[String],
	conn: &ConnContext,
	module_id: &str,
) -> Vec<ContextEntry> {
	if inspects.is_empty() {
		return Vec::new();
	}

	// Snapshot the TLS state once if any path needs it, so the per-
	// path arms don't re-acquire the parking_lot::Mutex for each of
	// the eight `conn.tls.peer_cert.*` siblings. `conn.alpn` is also
	// sourced from `TlsInfo` even though its path doesn't share the
	// `conn.tls.` prefix.
	let tls_snapshot: Option<TlsInfo> =
		if inspects.iter().any(|p| p == "conn.alpn" || p.starts_with("conn.tls")) {
			conn.tls.lock().clone()
		} else {
			None
		};

	let mut out = Vec::with_capacity(inspects.len());
	for path in inspects {
		match pack_one(path, conn, tls_snapshot.as_ref()) {
			Some(value) => out.push(ContextEntry { path: path.clone(), value }),
			None => warn_pack_unimplemented_once(module_id, path),
		}
	}
	out
}

fn pack_one(path: &str, conn: &ConnContext, tls: Option<&TlsInfo>) -> Option<ContextValue> {
	let v = match path {
		"conn.peer_ip" => ContextValue::Text(conn.remote.ip().to_string()),
		"conn.peer_port" => ContextValue::Uint64(u64::from(conn.remote.port())),
		"conn.local_ip" => ContextValue::Text(conn.local.ip().to_string()),
		"conn.local_port" => ContextValue::Uint64(u64::from(conn.local.port())),
		"conn.transport" => ContextValue::Text(transport_str(conn.transport).to_owned()),
		"conn.id" => ContextValue::Text(format!("{:016x}", conn.id.0)),
		"conn.accept_unix_ms" => ContextValue::Uint64(instant_to_unix_ms(conn.entered_at)),

		"conn.alpn" => ContextValue::Text(
			tls
				.and_then(|t| t.alpn.as_deref())
				.map(|b| String::from_utf8_lossy(b).into_owned())
				.unwrap_or_default(),
		),
		"conn.tls.version" => {
			ContextValue::Text(tls.and_then(|t| t.version).map_or("", tls_version_str).to_owned())
		}
		"conn.tls.sni" => ContextValue::Text(
			tls.and_then(|t| t.sni.as_deref()).map(str::to_ascii_lowercase).unwrap_or_default(),
		),

		"conn.tls.peer_cert" => ContextValue::Bytes(
			tls.and_then(|t| t.peer_cert.as_ref()).map(|c| c.leaf_der.to_vec()).unwrap_or_default(),
		),
		"conn.tls.peer_cert.present" => {
			ContextValue::Boolean(tls.and_then(|t| t.peer_cert.as_ref()).is_some())
		}
		"conn.tls.peer_cert.subject_cn" => ContextValue::Text(
			tls.and_then(|t| t.peer_cert.as_ref()).and_then(|c| c.subject_cn.clone()).unwrap_or_default(),
		),
		"conn.tls.peer_cert.san_dns" => ContextValue::ListText(
			tls.and_then(|t| t.peer_cert.as_ref()).map(|c| c.san_dns.clone()).unwrap_or_default(),
		),
		"conn.tls.peer_cert.fingerprint_sha256" => ContextValue::Text(
			tls
				.and_then(|t| t.peer_cert.as_ref())
				.map(|c| c.fingerprint_sha256.clone())
				.unwrap_or_default(),
		),
		"conn.tls.peer_cert.spki_sha256" => ContextValue::Text(
			tls.and_then(|t| t.peer_cert.as_ref()).map(|c| c.spki_sha256.clone()).unwrap_or_default(),
		),
		"conn.tls.peer_cert.issuer_cn" => ContextValue::Text(
			tls.and_then(|t| t.peer_cert.as_ref()).and_then(|c| c.issuer_cn.clone()).unwrap_or_default(),
		),
		"conn.tls.peer_cert.serial" => ContextValue::Text(
			tls.and_then(|t| t.peer_cert.as_ref()).map(|c| c.serial.clone()).unwrap_or_default(),
		),

		// Grammar-valid request / response path — load-time validation
		// admitted it but the host pack path defers them.
		_ => return None,
	};
	Some(v)
}

fn transport_str(t: Transport) -> &'static str {
	match t {
		Transport::Tcp => "tcp",
		Transport::Udp => "udp",
	}
}

fn tls_version_str(v: TlsVersion) -> &'static str {
	match v {
		TlsVersion::Tls12 => "1.2",
		TlsVersion::Tls13 => "1.3",
	}
}

/// Convert a monotonic `Instant` to unix-epoch milliseconds via the
/// current wall clock. Fresh per call rather than a cached
/// (`Instant`, `SystemTime`) pair, so an NTP wall-clock adjustment
/// propagates immediately without skewing already-loaded references.
/// Per-call cost is two clock reads, dwarfed by a single wasm
/// dispatch.
fn instant_to_unix_ms(t: Instant) -> u64 {
	let now_inst = Instant::now();
	let now_sys = SystemTime::now();
	let elapsed = now_inst.saturating_duration_since(t);
	now_sys
		.checked_sub(elapsed)
		.and_then(|s| s.duration_since(UNIX_EPOCH).ok())
		.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn warn_pack_unimplemented_once(module_id: &str, path: &str) {
	static EMITTED: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
	let set = EMITTED.get_or_init(|| Mutex::new(HashSet::new()));
	let key = (module_id.to_owned(), path.to_owned());
	let mut guard = set.lock().expect("warn-cache mutex");
	if guard.insert(key) {
		tracing::warn!(
			module_id,
			path,
			"inspects path declared but not packed by host \
			 (request/response-level paths are deferred — connection-level only for now)",
		);
	}
}

#[cfg(test)]
mod tests {
	use std::net::SocketAddr;
	use std::sync::{Arc, OnceLock as StdOnceLock};
	use std::time::Instant as StdInstant;

	use parking_lot::Mutex as PLMutex;
	use vane_core::{ConnContext, ConnId, PeerCertificate, TlsInfo, TlsVersion, Transport};

	use super::*;

	fn conn_with(
		remote: &str,
		local: &str,
		transport: Transport,
		tls: Option<TlsInfo>,
	) -> Arc<ConnContext> {
		Arc::new(ConnContext {
			id: ConnId(0x0bad_f00d_dead_beef),
			remote: remote.parse::<SocketAddr>().expect("remote parse"),
			local: local.parse::<SocketAddr>().expect("local parse"),
			transport,
			entered_at: StdInstant::now(),
			tls: PLMutex::new(tls),
			http_version: StdOnceLock::new(),
			user: PLMutex::new(http::Extensions::new()),
		})
	}

	fn pack_single(path: &str, conn: &ConnContext) -> Option<ContextValue> {
		let v = pack_context(&[path.to_owned()], conn, "test-module");
		assert!(v.len() <= 1, "single path packed into multiple entries: {v:?}");
		v.into_iter().next().map(|e| {
			assert_eq!(e.path, path);
			e.value
		})
	}

	fn assert_text(v: ContextValue, want: &str) {
		match v {
			ContextValue::Text(s) => assert_eq!(s, want),
			other => panic!("expected Text({want:?}), got {other:?}"),
		}
	}

	fn assert_uint64(v: ContextValue, want: u64) {
		match v {
			ContextValue::Uint64(n) => assert_eq!(n, want),
			other => panic!("expected Uint64({want}), got {other:?}"),
		}
	}

	fn assert_boolean(v: ContextValue, want: bool) {
		match v {
			ContextValue::Boolean(b) => assert_eq!(b, want),
			other => panic!("expected Boolean({want}), got {other:?}"),
		}
	}

	fn assert_bytes(v: ContextValue, want: &[u8]) {
		match v {
			ContextValue::Bytes(b) => assert_eq!(b, want),
			other => panic!("expected Bytes(..), got {other:?}"),
		}
	}

	fn assert_list_text(v: ContextValue, want: &[&str]) {
		match v {
			ContextValue::ListText(items) => {
				assert_eq!(items.len(), want.len());
				for (got, w) in items.iter().zip(want) {
					assert_eq!(got, w);
				}
			}
			other => panic!("expected ListText, got {other:?}"),
		}
	}

	#[test]
	fn empty_inspects_returns_empty_vec() {
		let conn = conn_with("127.0.0.1:1234", "10.0.0.1:443", Transport::Tcp, None);
		assert!(pack_context(&[], &conn, "m").is_empty());
	}

	#[test]
	fn conn_l4_paths_pack_correctly() {
		let conn = conn_with("198.51.100.7:55001", "192.0.2.10:443", Transport::Tcp, None);
		assert_text(pack_single("conn.peer_ip", &conn).expect("present"), "198.51.100.7");
		assert_uint64(pack_single("conn.peer_port", &conn).expect("present"), 55001);
		assert_text(pack_single("conn.local_ip", &conn).expect("present"), "192.0.2.10");
		assert_uint64(pack_single("conn.local_port", &conn).expect("present"), 443);
		assert_text(pack_single("conn.transport", &conn).expect("present"), "tcp");
		assert_text(pack_single("conn.id", &conn).expect("present"), "0badf00ddeadbeef");
	}

	#[test]
	fn conn_transport_udp_renders_lowercase() {
		let conn = conn_with("127.0.0.1:7", "127.0.0.1:53", Transport::Udp, None);
		assert_text(pack_single("conn.transport", &conn).expect("present"), "udp");
	}

	#[test]
	fn conn_accept_unix_ms_is_close_to_wall_clock_now() {
		let conn = conn_with("127.0.0.1:1", "127.0.0.1:1", Transport::Tcp, None);
		let now_ms =
			u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).expect("now").as_millis())
				.expect("u64 fit");
		match pack_single("conn.accept_unix_ms", &conn).expect("present") {
			ContextValue::Uint64(ms) => {
				// entered_at sampled inside conn_with a few microseconds ago;
				// allow a generous bound to defeat CI scheduling jitter.
				let diff = now_ms.abs_diff(ms);
				assert!(diff < 1_000, "accept_unix_ms {ms} too far from now {now_ms}");
			}
			other => panic!("expected Uint64, got {other:?}"),
		}
	}

	#[test]
	fn tls_paths_with_no_tls_info_pack_as_spec_empty_values() {
		let conn = conn_with("127.0.0.1:1", "127.0.0.1:1", Transport::Tcp, None);
		assert_text(pack_single("conn.alpn", &conn).expect("present"), "");
		assert_text(pack_single("conn.tls.version", &conn).expect("present"), "");
		assert_text(pack_single("conn.tls.sni", &conn).expect("present"), "");
		assert_bytes(pack_single("conn.tls.peer_cert", &conn).expect("present"), &[]);
		assert_boolean(pack_single("conn.tls.peer_cert.present", &conn).expect("present"), false);
		assert_text(pack_single("conn.tls.peer_cert.subject_cn", &conn).expect("present"), "");
		assert_list_text(pack_single("conn.tls.peer_cert.san_dns", &conn).expect("present"), &[]);
		assert_text(pack_single("conn.tls.peer_cert.fingerprint_sha256", &conn).expect("present"), "");
		assert_text(pack_single("conn.tls.peer_cert.spki_sha256", &conn).expect("present"), "");
		assert_text(pack_single("conn.tls.peer_cert.issuer_cn", &conn).expect("present"), "");
		assert_text(pack_single("conn.tls.peer_cert.serial", &conn).expect("present"), "");
	}

	#[test]
	fn tls_scalar_paths_pack_from_snapshot() {
		let tls = TlsInfo {
			sni: Some("Example.COM".to_owned()),
			alpn: Some(b"h2".to_vec()),
			version: Some(TlsVersion::Tls13),
			peer_cert: None,
			zero_rtt_used: false,
		};
		let conn = conn_with("127.0.0.1:1", "127.0.0.1:1", Transport::Tcp, Some(tls));
		assert_text(pack_single("conn.alpn", &conn).expect("present"), "h2");
		assert_text(pack_single("conn.tls.version", &conn).expect("present"), "1.3");
		// SNI is canonicalised to ASCII-lowercase per spec.
		assert_text(pack_single("conn.tls.sni", &conn).expect("present"), "example.com");
	}

	#[test]
	fn tls_peer_cert_paths_pack_from_snapshot() {
		let cert = PeerCertificate {
			leaf_der: bytes::Bytes::from_static(b"\x30\x82\x01\x00fake-der"),
			subject_cn: Some("client.example".to_owned()),
			san_dns: vec!["api.example".to_owned(), "edge.example".to_owned()],
			fingerprint_sha256: "deadbeef".to_owned(),
			spki_sha256: "feedface".to_owned(),
			issuer_cn: Some("Example CA".to_owned()),
			serial: "1234abcd".to_owned(),
		};
		let tls = TlsInfo { peer_cert: Some(Arc::new(cert)), ..TlsInfo::default() };
		let conn = conn_with("127.0.0.1:1", "127.0.0.1:1", Transport::Tcp, Some(tls));

		assert_bytes(
			pack_single("conn.tls.peer_cert", &conn).expect("present"),
			b"\x30\x82\x01\x00fake-der",
		);
		assert_boolean(pack_single("conn.tls.peer_cert.present", &conn).expect("present"), true);
		assert_text(
			pack_single("conn.tls.peer_cert.subject_cn", &conn).expect("present"),
			"client.example",
		);
		assert_list_text(
			pack_single("conn.tls.peer_cert.san_dns", &conn).expect("present"),
			&["api.example", "edge.example"],
		);
		assert_text(
			pack_single("conn.tls.peer_cert.fingerprint_sha256", &conn).expect("present"),
			"deadbeef",
		);
		assert_text(pack_single("conn.tls.peer_cert.spki_sha256", &conn).expect("present"), "feedface");
		assert_text(pack_single("conn.tls.peer_cert.issuer_cn", &conn).expect("present"), "Example CA");
		assert_text(pack_single("conn.tls.peer_cert.serial", &conn).expect("present"), "1234abcd");
	}

	#[test]
	fn mixed_inspects_pack_every_known_path() {
		let conn = conn_with("198.51.100.7:55001", "192.0.2.10:443", Transport::Tcp, None);
		let inspects =
			vec!["conn.peer_ip".to_owned(), "conn.transport".to_owned(), "conn.id".to_owned()];
		let entries = pack_context(&inspects, &conn, "m");
		assert_eq!(entries.len(), 3);
		let by_path: std::collections::HashMap<_, _> =
			entries.iter().map(|e| (e.path.as_str(), &e.value)).collect();
		assert!(by_path.contains_key("conn.peer_ip"));
		assert!(by_path.contains_key("conn.transport"));
		assert!(by_path.contains_key("conn.id"));
	}

	#[test]
	fn request_response_paths_are_skipped_not_packed() {
		// pack_one returns None for any path outside the connection
		// table — pack_context omits the entry. The warn-once side
		// effect is logged via tracing; we don't capture it here, but
		// the absence-from-output assertion is the load-bearing one
		// (plugins must not see a deferred path packed as an empty
		// value, which would mask "field unimplemented" as "field
		// absent").
		let conn = conn_with("127.0.0.1:1", "127.0.0.1:1", Transport::Tcp, None);
		let inspects = vec![
			"http.method".to_owned(),
			"http.uri.path".to_owned(),
			"http.header.authorization".to_owned(),
			"conn.peer_ip".to_owned(),
		];
		let entries = pack_context(&inspects, &conn, "m");
		assert_eq!(entries.len(), 1, "only conn.peer_ip should be packed");
		assert_eq!(entries[0].path, "conn.peer_ip");
	}
}
