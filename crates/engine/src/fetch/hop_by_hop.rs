//! RFC 7230 §6.1 / RFC 9110 hop-by-hop header stripping for the
//! reverse-proxy fetch path.
//!
//! Hop-by-hop headers describe the **single** TCP / TLS hop between
//! two HTTP peers; a proxy must never propagate them onward. Vane
//! is a reverse proxy, so both the request (client → upstream) and
//! the response (upstream → client) must be sanitised. Without this
//! pass, a hostile client can leak arbitrary inbound headers to the
//! upstream by listing them in `Connection:` (e.g. `Connection:
//! x-secret` → strip `x-secret` from the client view but forward to
//! the upstream); symmetrically, a hostile upstream can leak its
//! internal state to clients.
//!
//! The set has two parts:
//!
//! - A **static set** from RFC 7230 §6.1 + the historical
//!   `Proxy-Connection` (non-standard but ubiquitous and meaningful
//!   only for the single hop): every name on this list is removed
//!   regardless of any `Connection:` directive.
//! - A **dynamic set** drawn from `Connection:` tokens (RFC 7230
//!   §6.1: "Connection lists per-hop names that the recipient must
//!   not propagate"). Each `Connection:` directive's tokens are
//!   added to the strip set for this hop only.
//!
//! ## WebSocket exception
//!
//! [RFC 6455 §1.3 + §11.3] makes `Upgrade` + `Connection: Upgrade`
//! the *transport* of the WebSocket handshake; they are
//! hop-by-hop in the HTTP layer but the WebSocket layer requires
//! both peers to see them. [`strip_hop_by_hop_request`] preserves
//! `Connection`, `Upgrade`, and the `Sec-WebSocket-*` family when
//! the request is recognisably a WebSocket handshake, and rewrites
//! `Connection:` so the only remaining token is `upgrade` —
//! defeating the `Connection: upgrade, x-evil` smuggling vector.
//! On the response side, the same exception fires only for `101
//! Switching Protocols`, matching the handshake response shape.
//!
//! ## Why not let `hyper-util` handle this
//!
//! `hyper-util`'s `legacy::Client` reshapes a few headers on the way
//! out (notably `Transfer-Encoding`) but does **not** read the
//! `Connection:` token list to compute the dynamic strip set, and
//! certainly does nothing on the inbound side. Stripping has to
//! happen explicitly at the fetch boundary.
//!
//! See [`spec/crates/engine.md` § _Hop-by-hop sanitisation_].

use http::HeaderMap;
use http::header::{
	CONNECTION, HeaderName, HeaderValue, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER,
	TRANSFER_ENCODING, UPGRADE,
};

/// RFC 7230 §6.1 connection-options. Tokens that name a *behaviour*
/// for the immediate hop rather than a header to remove. These must
/// survive the strip and are written back through the rewritten
/// `Connection:` header so hyper's H1 framing layer can act on them.
fn is_connection_option(tok: &str) -> bool {
	tok.eq_ignore_ascii_case("close")
		|| tok.eq_ignore_ascii_case("keep-alive")
		|| tok.eq_ignore_ascii_case("upgrade")
}

/// Static hop-by-hop header set from RFC 7230 §6.1 plus the de-facto
/// `Proxy-Connection`. Note `Connection` and `Upgrade` are **not**
/// in this set — they are handled separately because their values
/// carry connection-options (`close`, `keep-alive`, `upgrade`) that
/// must survive the strip and continue to govern the immediate hop's
/// framing behaviour (per RFC 7230 §6.1 and hyper's H1 framing).
fn static_hop_by_hop() -> [&'static HeaderName; 7] {
	use std::sync::OnceLock;
	static KEEP_ALIVE: OnceLock<HeaderName> = OnceLock::new();
	static PROXY_CONNECTION: OnceLock<HeaderName> = OnceLock::new();
	let keep_alive = KEEP_ALIVE.get_or_init(|| HeaderName::from_static("keep-alive"));
	let proxy_conn = PROXY_CONNECTION.get_or_init(|| HeaderName::from_static("proxy-connection"));
	[
		keep_alive,
		&PROXY_AUTHENTICATE,
		&PROXY_AUTHORIZATION,
		proxy_conn,
		&TE,
		&TRAILER,
		// Transfer-Encoding is hop-by-hop in RFC terms but hyper's
		// H1 encoder reads it to decide framing. Stripping it after
		// a chunked body has already been demuxed would leave the
		// downstream encoder without framing context, so the strip
		// happens via the executor's normal re-emit path: hyper
		// reconstructs framing from `Body::Stream` shape, not from
		// this header. The header on an `IncomingResponse` is
		// already an after-the-fact label; we drop it so it does
		// not propagate back onto a different transport.
		&TRANSFER_ENCODING,
	]
}

/// Decomposition of the `Connection:` header into two streams:
///
/// - `options` — the connection-option tokens (`close`, `keep-alive`,
///   `upgrade`) that drive framing behaviour for the immediate hop
///   and must continue to govern downstream encoders.
/// - `names` — header names listed for per-hop removal per RFC 7230
///   §6.1; each one must be stripped from the message after this
///   parse.
///
/// Returns empty parts when no `Connection:` is present. Malformed
/// tokens (anything that does not parse as a `HeaderName` and is not
/// a recognised connection-option) are silently dropped — a name
/// that fails to parse cannot match anything in the `HeaderMap`
/// anyway.
struct ConnectionParts {
	options: Vec<String>,
	names: Vec<HeaderName>,
}

fn parse_connection(headers: &HeaderMap) -> ConnectionParts {
	let mut options = Vec::new();
	let mut names = Vec::new();
	for v in &headers.get_all(CONNECTION) {
		let Ok(s) = v.to_str() else { continue };
		for tok in s.split(',') {
			let trimmed = tok.trim();
			if trimmed.is_empty() {
				continue;
			}
			if is_connection_option(trimmed) {
				let lower = trimmed.to_ascii_lowercase();
				if !options.iter().any(|o: &String| o == &lower) {
					options.push(lower);
				}
				continue;
			}
			if let Ok(name) = HeaderName::try_from(trimmed) {
				names.push(name);
			}
		}
	}
	ConnectionParts { options, names }
}

/// Rebuild the `Connection:` header on `headers` from `options`. If
/// `options` is empty, removes the header entirely; otherwise writes
/// a comma-separated value with the canonical lowercase form.
fn rewrite_connection(headers: &mut HeaderMap, options: &[String]) {
	if options.is_empty() {
		headers.remove(CONNECTION);
		return;
	}
	let value = options.join(", ");
	// All tokens are ASCII (`close` / `keep-alive` / `upgrade`), so
	// `HeaderValue::from_str` is infallible.
	let v = HeaderValue::from_str(&value).expect("connection-options are ASCII");
	headers.insert(CONNECTION, v);
}

/// Strip hop-by-hop headers from a request the proxy is about to
/// forward. Honours the WebSocket-upgrade exception when the request
/// is a recognisable `Upgrade: websocket` handshake.
///
/// Side effects: mutates `headers` in place.
pub(crate) fn strip_hop_by_hop_request(headers: &mut HeaderMap) {
	let ws = looks_like_websocket_request(headers);
	let parts = parse_connection(headers);
	for name in static_hop_by_hop() {
		headers.remove(name);
	}
	for name in &parts.names {
		// In WS mode the only legal `Connection:` token is `upgrade`;
		// names already excludes connection-options, so anything
		// listed here is a hop-by-hop bystander.
		headers.remove(name);
	}
	if ws {
		// Defeat `Connection: upgrade, x-evil` smuggling: the
		// outgoing `Connection:` carries exactly one `upgrade`
		// token. `Upgrade:` is preserved verbatim (set by the
		// client; we don't touch it).
		headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
	} else {
		// Non-WS request: rewrite `Connection:` to retain any
		// surviving connection-options (typically `close` /
		// `keep-alive`); these are framing directives for the
		// immediate hop and downstream encoders need to see them.
		rewrite_connection(headers, &parts.options);
		// `Upgrade:` is meaningful only when paired with a
		// `Connection: upgrade` token. Without the WS exception
		// firing, strip it.
		headers.remove(UPGRADE);
	}
}

/// Strip hop-by-hop headers from an upstream response the proxy is
/// about to forward to the client. `is_switching_protocols` flips on
/// the WebSocket exception (RFC 6455 §4.2.2 — the 101 response carries
/// `Connection: Upgrade` + `Upgrade: websocket`).
pub(crate) fn strip_hop_by_hop_response(headers: &mut HeaderMap, is_switching_protocols: bool) {
	let parts = parse_connection(headers);
	for name in static_hop_by_hop() {
		headers.remove(name);
	}
	for name in &parts.names {
		headers.remove(name);
	}
	if is_switching_protocols {
		headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
	} else {
		// Preserve `close` / `keep-alive` so hyper's H1 encoder on
		// the next hop reproduces the framing decision the upstream
		// signalled. Drop everything else from `Connection:` and
		// drop `Upgrade:` (only meaningful with a paired
		// `Connection: upgrade`, handled by the WS branch).
		rewrite_connection(headers, &parts.options);
		headers.remove(UPGRADE);
	}
}

/// Recognise a WebSocket handshake. The dispatch is intentionally
/// strict: both `Upgrade: websocket` and `Connection: upgrade` must
/// be present, matching RFC 6455 §4.1. A request missing either
/// signal is treated as a normal HTTP request and the full hop-by-hop
/// strip applies.
fn looks_like_websocket_request(headers: &HeaderMap) -> bool {
	let upgrade_is_ws = headers
		.get(UPGRADE)
		.and_then(|v| v.to_str().ok())
		.is_some_and(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case("websocket")));
	let connection_has_upgrade = headers
		.get_all(CONNECTION)
		.iter()
		.filter_map(|v| v.to_str().ok())
		.any(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade")));
	upgrade_is_ws && connection_has_upgrade
}

#[cfg(test)]
mod tests {
	use http::HeaderValue;

	use super::*;

	fn map_from(pairs: &[(&str, &str)]) -> HeaderMap {
		let mut h = HeaderMap::new();
		for (n, v) in pairs {
			h.append(HeaderName::try_from(*n).expect("name"), HeaderValue::from_str(v).expect("value"));
		}
		h
	}

	#[test]
	fn static_set_is_removed_from_request() {
		let mut h = map_from(&[
			("connection", "close"),
			("keep-alive", "timeout=15"),
			("proxy-authorization", "basic dXNlcjpwYXNz"),
			("te", "trailers"),
			("trailer", "etag"),
			("transfer-encoding", "chunked"),
			("upgrade", "h2c"),
			("proxy-connection", "keep-alive"),
			("x-keep", "keep-this"),
		]);
		strip_hop_by_hop_request(&mut h);
		// `Keep-Alive`, `Proxy-Authorization`, `TE`, `Trailer`,
		// `Transfer-Encoding`, `Proxy-Connection` are unconditional
		// hop-by-hop names. `Upgrade` is meaningful only with a
		// paired `Connection: upgrade`; since this request has
		// `Connection: close` instead, `Upgrade` is also dropped.
		for stripped in [
			"keep-alive",
			"proxy-authorization",
			"te",
			"trailer",
			"transfer-encoding",
			"upgrade",
			"proxy-connection",
		] {
			assert!(h.get(stripped).is_none(), "{stripped} must be stripped");
		}
		// `Connection: close` survives as a connection-option — it
		// drives framing on the immediate hop.
		assert_eq!(h.get("connection").map(HeaderValue::as_bytes), Some(b"close" as &[u8]));
		assert_eq!(h.get("x-keep").map(HeaderValue::as_bytes), Some(b"keep-this" as &[u8]));
	}

	#[test]
	fn dynamic_connection_tokens_are_stripped_while_close_option_survives() {
		// Classic smuggling vector: leak inbound `x-private` upstream
		// via `Connection: x-private`. The named headers must go;
		// the `close` connection-option must remain in a rewritten
		// `Connection:` so the downstream encoder still sees the
		// framing directive.
		let mut h = map_from(&[
			("connection", "close, x-private, X-Other"),
			("x-private", "secret"),
			("x-other", "more-secret"),
			("x-keep", "keep-this"),
		]);
		strip_hop_by_hop_request(&mut h);
		assert_eq!(
			h.get("connection").map(HeaderValue::as_bytes),
			Some(b"close" as &[u8]),
			"close survives, named smuggled headers stripped",
		);
		assert!(h.get("x-private").is_none(), "Connection-listed x-private must be stripped");
		assert!(
			h.get("x-other").is_none(),
			"Connection-listed X-Other must be stripped (case-insensitive)"
		);
		assert_eq!(h.get("x-keep").map(HeaderValue::as_bytes), Some(b"keep-this" as &[u8]));
	}

	#[test]
	fn multiple_connection_headers_each_contribute_tokens() {
		let mut h = HeaderMap::new();
		h.append("connection", HeaderValue::from_static("close, x-one"));
		h.append("connection", HeaderValue::from_static("keep-alive, x-two"));
		h.append("x-one", HeaderValue::from_static("a"));
		h.append("x-two", HeaderValue::from_static("b"));
		strip_hop_by_hop_request(&mut h);
		assert!(h.get("x-one").is_none(), "first Connection list must contribute x-one");
		assert!(h.get("x-two").is_none(), "second Connection list must contribute x-two");
		// Connection-options across both headers survive as a single
		// rewritten value. Both options are present; the order
		// follows first-seen.
		let surviving = h.get("connection").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
		assert!(
			surviving.contains("close") && surviving.contains("keep-alive"),
			"options preserved: {surviving}"
		);
	}

	#[test]
	fn malformed_connection_token_is_silently_ignored() {
		// `bad token` contains a space, which is not a valid
		// header-name char per RFC 9110 §5.6.2. The parser must
		// drop the malformed token without panicking and apply
		// strip to the legitimate siblings around it. The
		// malformed token cannot itself name a header that exists
		// in a `HeaderMap` (the http crate rejects such names on
		// insert), so the assertion is "the parser does not crash
		// and surrounding tokens still take effect".
		let mut h = map_from(&[
			("connection", "x-allowed, bad token, x-second"),
			("x-allowed", "1"),
			("x-second", "2"),
		]);
		strip_hop_by_hop_request(&mut h);
		assert!(h.get("x-allowed").is_none(), "valid pre-token still applied");
		assert!(h.get("x-second").is_none(), "valid post-token still applied");
	}

	#[test]
	fn websocket_request_preserves_upgrade_path_and_sanitises_connection() {
		let mut h = map_from(&[
			("upgrade", "websocket"),
			("connection", "upgrade, x-evil"),
			("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
			("sec-websocket-version", "13"),
			("x-evil", "leak-attempt"),
			("transfer-encoding", "chunked"),
		]);
		strip_hop_by_hop_request(&mut h);
		assert_eq!(
			h.get("connection").map(HeaderValue::as_bytes),
			Some(b"upgrade" as &[u8]),
			"Connection must be rewritten to the single canonical `upgrade` token",
		);
		assert_eq!(
			h.get("upgrade").map(HeaderValue::as_bytes),
			Some(b"websocket" as &[u8]),
			"Upgrade must survive the WebSocket exception",
		);
		assert_eq!(
			h.get("sec-websocket-key").map(HeaderValue::as_bytes),
			Some(b"dGhlIHNhbXBsZSBub25jZQ==" as &[u8]),
		);
		assert_eq!(h.get("sec-websocket-version").map(HeaderValue::as_bytes), Some(b"13" as &[u8]),);
		assert!(h.get("x-evil").is_none(), "Connection-listed x-evil must still be stripped");
		assert!(h.get("transfer-encoding").is_none(), "static hop-by-hop unaffected by ws exception");
	}

	#[test]
	fn non_websocket_request_does_not_trigger_exception() {
		// `Upgrade: websocket` without `Connection: upgrade` is not a
		// valid WebSocket handshake; treat as a normal request and
		// strip the `Upgrade:` header. `Connection: close` is a
		// connection-option and stays.
		let mut h = map_from(&[("upgrade", "websocket"), ("connection", "close")]);
		strip_hop_by_hop_request(&mut h);
		assert!(h.get("upgrade").is_none(), "Upgrade dropped without paired Connection: upgrade");
		assert_eq!(h.get("connection").map(HeaderValue::as_bytes), Some(b"close" as &[u8]));
	}

	#[test]
	fn static_set_is_removed_from_response() {
		let mut h = map_from(&[
			("connection", "close, x-leak"),
			("keep-alive", "timeout=5"),
			("proxy-authenticate", "Basic realm=\"upstream\""),
			("transfer-encoding", "chunked"),
			("x-leak", "internal-state"),
			("x-keep", "ok"),
		]);
		strip_hop_by_hop_response(&mut h, false);
		assert_eq!(
			h.get("connection").map(HeaderValue::as_bytes),
			Some(b"close" as &[u8]),
			"Connection: close survives — it tells the next hop's encoder to close after the response",
		);
		assert!(h.get("keep-alive").is_none());
		assert!(h.get("proxy-authenticate").is_none());
		assert!(h.get("transfer-encoding").is_none());
		assert!(h.get("x-leak").is_none(), "Connection-listed x-leak must be stripped from response");
		assert_eq!(h.get("x-keep").map(HeaderValue::as_bytes), Some(b"ok" as &[u8]));
	}

	#[test]
	fn switching_protocols_response_preserves_upgrade_path() {
		let mut h = map_from(&[
			("connection", "upgrade, x-evil"),
			("upgrade", "websocket"),
			("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
			("x-evil", "leak"),
		]);
		strip_hop_by_hop_response(&mut h, true);
		assert_eq!(h.get("connection").map(HeaderValue::as_bytes), Some(b"upgrade" as &[u8]));
		assert_eq!(h.get("upgrade").map(HeaderValue::as_bytes), Some(b"websocket" as &[u8]));
		assert_eq!(
			h.get("sec-websocket-accept").map(HeaderValue::as_bytes),
			Some(b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=" as &[u8]),
		);
		assert!(h.get("x-evil").is_none());
	}

	#[test]
	fn casing_does_not_affect_strip_decision() {
		let mut h = map_from(&[("Connection", "X-Private, ClOsE"), ("X-Private", "secret")]);
		strip_hop_by_hop_request(&mut h);
		assert!(h.get("x-private").is_none(), "case-insensitive token match must strip");
		// `close` survives via the canonical lowercase form.
		assert_eq!(h.get("connection").map(HeaderValue::as_bytes), Some(b"close" as &[u8]));
	}
}
