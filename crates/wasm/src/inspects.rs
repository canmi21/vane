//! Inspects path grammar — the canonical list of `inspects` field
//! paths a plugin may declare.
//!
//! Mirrors `spec/wasm-abi.md` § _Path grammar — connection-level_ (connection table) and
//! `spec/crates/core.md` (request / response
//! table). Any addition here must update the spec table and the host
//! pack site in the same commit, or load-time validation diverges
//! from the documented grammar.

/// Connection-level paths from `spec/wasm-abi.md` § _Path grammar — connection-level_.
/// Static-fixed: every path packs from a `ConnContext` field.
const CONN_PATHS: &[&str] = &[
	"conn.peer_ip",
	"conn.peer_port",
	"conn.local_ip",
	"conn.local_port",
	"conn.transport",
	"conn.alpn",
	"conn.id",
	"conn.accept_unix_ms",
	"conn.tls.version",
	"conn.tls.sni",
	"conn.tls.peer_cert",
	"conn.tls.peer_cert.present",
	"conn.tls.peer_cert.subject_cn",
	"conn.tls.peer_cert.san_dns",
	"conn.tls.peer_cert.fingerprint_sha256",
	"conn.tls.peer_cert.spki_sha256",
	"conn.tls.peer_cert.issuer_cn",
	"conn.tls.peer_cert.serial",
];

/// Request / response-level paths mirrored from
/// `spec/crates/core.md`. These pass load-time
/// validation as a known grammar; the current host pack path defers
/// them, and dispatch logs a warn-once per `(module_id, path)` when
/// one is declared.
const REQ_RESP_STATIC_PATHS: &[&str] =
	&["http.method", "http.uri.path", "http.uri.query", "http.body"];

/// Validate a single `inspects` path string.
///
/// Static membership for fixed paths above; the dynamic
/// `http.header.<name>` form is accepted with a syntactic check on
/// the suffix (RFC 7230 `token`). Unknown paths return `false` so the
/// caller can reject the plugin at load time — the alternative is a
/// silently-empty `context` entry the plugin author expected
/// populated.
#[must_use]
pub fn validate_inspects_path(path: &str) -> bool {
	if CONN_PATHS.contains(&path) || REQ_RESP_STATIC_PATHS.contains(&path) {
		return true;
	}
	if let Some(rest) = path.strip_prefix("http.header.") {
		return is_token(rest);
	}
	false
}

/// Whether the path is request- or response-level. Used by the
/// dispatch path to skip packing (current scope is connection-level
/// only) and emit a warn-once.
#[must_use]
pub fn is_request_or_response_path(path: &str) -> bool {
	REQ_RESP_STATIC_PATHS.contains(&path) || path.starts_with("http.header.")
}

/// RFC 7230 `token`: one or more `tchar`, where `tchar` is alphanumeric
/// plus the punctuation set `! # $ % & ' * + - . ^ _ | ~` (and
/// backtick). Header field-names and (per RFC 6265 §4.1.1)
/// cookie-names share this grammar.
fn is_token(s: &str) -> bool {
	!s.is_empty()
		&& s.bytes().all(|b| {
			matches!(b,
			b'!' | b'#'..=b'\'' | b'*' | b'+' | b'-' | b'.' |
			b'0'..=b'9' | b'A'..=b'Z' | b'^'..=b'`' | b'a'..=b'z' | b'|' | b'~')
		})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_connection_path_in_table_validates() {
		for path in CONN_PATHS {
			assert!(validate_inspects_path(path), "conn path rejected: {path}");
		}
	}

	#[test]
	fn every_request_response_static_path_validates() {
		for path in REQ_RESP_STATIC_PATHS {
			assert!(validate_inspects_path(path), "req/resp path rejected: {path}");
		}
	}

	#[test]
	fn dynamic_header_paths_validate_with_well_formed_names() {
		for name in ["authorization", "x-custom-id", "Content-Type", "X-Foo.Bar_baz"] {
			let path = format!("http.header.{name}");
			assert!(validate_inspects_path(&path), "header path rejected: {path}");
		}
	}

	#[test]
	fn dynamic_header_path_rejects_empty_suffix() {
		assert!(!validate_inspects_path("http.header."));
	}

	#[test]
	fn dynamic_header_path_rejects_non_token_chars() {
		// Space, comma, slash, colon, semicolon, control chars are not
		// token chars per RFC 7230.
		for bad in ["bad value", "bad,name", "bad/name", "bad:name", "bad;name", "bad\tname"] {
			let path = format!("http.header.{bad}");
			assert!(!validate_inspects_path(&path), "header path wrongly accepted: {path}");
		}
	}

	#[test]
	fn unknown_paths_rejected() {
		// Unknown top-level / typo / out-of-grammar paths fail load.
		for bad in [
			"",
			"conn.unknown",
			"conn.tls.peer_cert.unknown",
			"conn..peer_ip",
			"http.cookie.session_id",
			"http.host",
			"http.scheme",
			"http.path",
			"random.text",
		] {
			assert!(!validate_inspects_path(bad), "unknown path wrongly accepted: {bad:?}");
		}
	}

	#[test]
	fn is_request_or_response_path_classifies_correctly() {
		assert!(is_request_or_response_path("http.method"));
		assert!(is_request_or_response_path("http.body"));
		assert!(is_request_or_response_path("http.header.authorization"));
		assert!(!is_request_or_response_path("conn.peer_ip"));
		assert!(!is_request_or_response_path("conn.tls.peer_cert.spki_sha256"));
		assert!(!is_request_or_response_path("random.text"));
	}
}
