//! Serialize an HTTP request into the RFC 3875 §4 environment-
//! variable list a CGI child expects on `execve`.
//!
//! The output of [`build_env`] is a `Vec<(String, String)>` ready
//! to feed into `std::process::Command::envs(...)`. Pair this crate
//! with `cgi-response` when you need both directions of a CGI
//! gateway.
//!
//! See the crate-level README for the full motivation and what's
//! intentionally not covered here.

use std::net::SocketAddr;
use std::path::Path;

/// RFC 3875 §4.1 required variables. An operator-supplied env entry
/// using one of these keys collides with what [`build_env`]
/// computes per request.
pub const RFC_3875_REQUIRED: &[&str] = &[
	"CONTENT_LENGTH",
	"CONTENT_TYPE",
	"GATEWAY_INTERFACE",
	"PATH_INFO",
	"PATH_TRANSLATED",
	"QUERY_STRING",
	"REMOTE_ADDR",
	"REMOTE_HOST",
	"REQUEST_METHOD",
	"SCRIPT_NAME",
	"SERVER_NAME",
	"SERVER_PORT",
	"SERVER_PROTOCOL",
	"SERVER_SOFTWARE",
];

/// Common-extension variables (not in RFC 3875 but ubiquitous, set
/// by Apache / nginx / lighttpd by default).
pub const COMMON_EXTENSIONS: &[&str] =
	&["REMOTE_PORT", "REQUEST_URI", "REQUEST_SCHEME", "HTTPS", "DOCUMENT_URI"];

/// Returns `true` when `key` collides with a value [`build_env`]
/// computes per request:
///
/// * Anything starting with `HTTP_` — the request-header
///   passthrough namespace.
/// * One of [`RFC_3875_REQUIRED`].
/// * One of [`COMMON_EXTENSIONS`].
///
/// Use this in your operator-config validator to reject `extra_env`
/// entries that would silently get clobbered.
#[must_use]
pub fn is_reserved_env_key(key: &str) -> bool {
	if key.starts_with("HTTP_") {
		return true;
	}
	RFC_3875_REQUIRED.contains(&key) || COMMON_EXTENSIONS.contains(&key)
}

/// All of the inputs [`build_env`] needs to produce a CGI
/// environment. Borrowed where possible to avoid forcing the
/// caller into allocations.
pub struct CgiRequestMeta<'a> {
	/// HTTP method as text (e.g. `"GET"`, `"POST"`).
	pub method: &'a str,
	/// Request URI path, no scheme / authority. Becomes part of the
	/// `DOCUMENT_URI` and the source for `PATH_INFO` after
	/// stripping `script_name`.
	pub path: &'a str,
	/// Request URI query, **without** the leading `?`. `None` when
	/// the original URI carried no query string at all.
	pub query: Option<&'a str>,
	/// Inbound request headers. Mapped to the `HTTP_*` passthrough
	/// namespace, with `Content-Length` / `Content-Type` lifted out
	/// to their dedicated CGI variables and `Host` used as a
	/// `SERVER_NAME` fallback when present.
	pub headers: &'a http::HeaderMap,
	/// Operator-configured `SCRIPT_NAME` — the URI prefix that
	/// identifies the script. `path - script_name` becomes
	/// `PATH_INFO`.
	pub script_name: &'a str,
	/// Operator-configured working directory. `working_dir +
	/// path_info` becomes `PATH_TRANSLATED`.
	pub working_dir: &'a Path,
	/// Server-side socket address (the one the listener accepted
	/// on). `SERVER_PORT` is taken from this; the IP is the
	/// fallback when the request has no `Host` header.
	pub server_addr: SocketAddr,
	/// Client-side socket address. Drives `REMOTE_ADDR` /
	/// `REMOTE_HOST` / `REMOTE_PORT`.
	pub remote_addr: SocketAddr,
	/// Whether the inbound connection terminated TLS at the
	/// listener. Sets `REQUEST_SCHEME` (`http` / `https`) and the
	/// `HTTPS=on` extension variable.
	pub is_tls: bool,
	/// `SERVER_SOFTWARE` value — the host's identification string,
	/// e.g. `"vane/0.10.4"` or `"myapp/1.0"`.
	pub server_software: &'a str,
	/// Operator-configured header names (case-insensitive) to drop
	/// from the `HTTP_*` passthrough. Useful for stripping
	/// secrets / internal headers before the child sees them.
	pub block_headers: &'a [String],
	/// Operator-supplied extra environment entries appended after
	/// the per-request set. Callers are expected to have validated
	/// these against [`is_reserved_env_key`] at config-load time.
	pub extra_env: &'a [(String, String)],
}

/// Build the RFC 3875 §4 environment-variable list for a CGI child
/// from an HTTP request meta. The output is ordered:
///
/// 1. RFC 3875 required variables in spec order.
/// 2. Common-extension variables.
/// 3. `HTTP_*` passthrough (inbound header → `HTTP_NAME` mapping,
///    minus `Content-Length` / `Content-Type` and any
///    `block_headers` entry).
/// 4. Operator extras (`extra_env`), appended verbatim.
///
/// The ordering inside category 3 follows whatever order the
/// `headers` map yields. CGI children typically read env via
/// `getenv(3)` (key-based) so the order is informational rather
/// than load-bearing — but consistent ordering helps when an
/// operator captures and diffs envs across runs.
#[must_use]
pub fn build_env(meta: &CgiRequestMeta<'_>) -> Vec<(String, String)> {
	let mut env: Vec<(String, String)> = Vec::with_capacity(32);

	let path_info = meta.path.strip_prefix(meta.script_name).unwrap_or(meta.path);
	let mut path_translated = meta.working_dir.to_path_buf();
	if !path_info.is_empty() {
		path_translated.push(path_info.trim_start_matches('/'));
	}
	let path_translated = path_translated.to_string_lossy().into_owned();
	let request_uri = match meta.query {
		Some(q) if !q.is_empty() => format!("{}?{}", meta.path, q),
		_ => meta.path.to_owned(),
	};

	let content_length = meta
		.headers
		.get(http::header::CONTENT_LENGTH)
		.and_then(|v| v.to_str().ok())
		.unwrap_or("0")
		.to_owned();
	let content_type = meta
		.headers
		.get(http::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.to_owned();
	let server_name = meta
		.headers
		.get(http::header::HOST)
		.and_then(|v| v.to_str().ok())
		.map_or_else(|| meta.server_addr.ip().to_string(), str::to_owned);

	// Category 1 — RFC 3875 §4.1.
	env.push(("CONTENT_LENGTH".to_owned(), content_length));
	env.push(("CONTENT_TYPE".to_owned(), content_type));
	env.push(("GATEWAY_INTERFACE".to_owned(), "CGI/1.1".to_owned()));
	env.push(("PATH_INFO".to_owned(), path_info.to_owned()));
	env.push(("PATH_TRANSLATED".to_owned(), path_translated));
	env.push(("QUERY_STRING".to_owned(), meta.query.unwrap_or("").to_owned()));
	env.push(("REMOTE_ADDR".to_owned(), meta.remote_addr.ip().to_string()));
	env.push(("REMOTE_HOST".to_owned(), meta.remote_addr.ip().to_string()));
	env.push(("REQUEST_METHOD".to_owned(), meta.method.to_owned()));
	env.push(("SCRIPT_NAME".to_owned(), meta.script_name.to_owned()));
	env.push(("SERVER_NAME".to_owned(), server_name));
	env.push(("SERVER_PORT".to_owned(), meta.server_addr.port().to_string()));
	env.push(("SERVER_PROTOCOL".to_owned(), "HTTP/1.1".to_owned()));
	env.push(("SERVER_SOFTWARE".to_owned(), meta.server_software.to_owned()));

	// Category 2 — common extensions.
	env.push(("REMOTE_PORT".to_owned(), meta.remote_addr.port().to_string()));
	env.push(("REQUEST_URI".to_owned(), request_uri));
	env.push((
		"REQUEST_SCHEME".to_owned(),
		if meta.is_tls { "https".to_owned() } else { "http".to_owned() },
	));
	if meta.is_tls {
		env.push(("HTTPS".to_owned(), "on".to_owned()));
	}
	env.push(("DOCUMENT_URI".to_owned(), meta.path.to_owned()));

	// Category 3 — `HTTP_*` passthrough.
	for (name, value) in meta.headers {
		let lower = name.as_str().to_ascii_lowercase();
		if lower == "content-length" || lower == "content-type" {
			continue;
		}
		if meta.block_headers.iter().any(|b| b.eq_ignore_ascii_case(name.as_str())) {
			continue;
		}
		let key = format!("HTTP_{}", name.as_str().to_ascii_uppercase().replace('-', "_"));
		let val = value.to_str().unwrap_or("").to_owned();
		env.push((key, val));
	}

	// Category 4 — operator extras.
	for (k, v) in meta.extra_env {
		env.push((k.clone(), v.clone()));
	}

	env
}

#[cfg(test)]
mod tests {
	use super::*;

	fn server() -> SocketAddr {
		"127.0.0.1:8080".parse().unwrap()
	}

	fn client() -> SocketAddr {
		"203.0.113.7:54321".parse().unwrap()
	}

	fn empty_headers() -> http::HeaderMap {
		http::HeaderMap::new()
	}

	fn meta<'a>(
		path: &'a str,
		query: Option<&'a str>,
		headers: &'a http::HeaderMap,
		script_name: &'a str,
		working_dir: &'a Path,
		block_headers: &'a [String],
		extra_env: &'a [(String, String)],
	) -> CgiRequestMeta<'a> {
		CgiRequestMeta {
			method: "GET",
			path,
			query,
			headers,
			script_name,
			working_dir,
			server_addr: server(),
			remote_addr: client(),
			is_tls: false,
			server_software: "test/1.0",
			block_headers,
			extra_env,
		}
	}

	fn lookup<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
		env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
	}

	#[test]
	fn reserved_env_keys_cover_rfc_required_and_extensions() {
		assert!(is_reserved_env_key("CONTENT_LENGTH"));
		assert!(is_reserved_env_key("REMOTE_ADDR"));
		assert!(is_reserved_env_key("HTTPS"));
		assert!(is_reserved_env_key("HTTP_USER_AGENT"));
		assert!(is_reserved_env_key("HTTP_X_CUSTOM"));
	}

	#[test]
	fn reserved_env_keys_pass_through_unrelated_names() {
		assert!(!is_reserved_env_key("APP_DEBUG"));
		assert!(!is_reserved_env_key("DATABASE_URL"));
		assert!(!is_reserved_env_key("Http_lower"));
	}

	#[test]
	fn build_env_splits_path_info_and_path_translated() {
		let headers = empty_headers();
		let env = build_env(&meta(
			"/cgi-bin/app.cgi/users/42",
			Some("sort=asc"),
			&headers,
			"/cgi-bin/app.cgi",
			Path::new("/var/www/cgi-bin"),
			&[],
			&[],
		));
		assert_eq!(lookup(&env, "SCRIPT_NAME"), Some("/cgi-bin/app.cgi"));
		assert_eq!(lookup(&env, "PATH_INFO"), Some("/users/42"));
		assert_eq!(lookup(&env, "PATH_TRANSLATED"), Some("/var/www/cgi-bin/users/42"));
		assert_eq!(lookup(&env, "QUERY_STRING"), Some("sort=asc"));
		assert_eq!(lookup(&env, "REQUEST_URI"), Some("/cgi-bin/app.cgi/users/42?sort=asc"));
		assert_eq!(lookup(&env, "DOCUMENT_URI"), Some("/cgi-bin/app.cgi/users/42"));
	}

	#[test]
	fn build_env_omits_query_marker_when_no_query() {
		let headers = empty_headers();
		let env = build_env(&meta(
			"/cgi-bin/app.cgi",
			None,
			&headers,
			"/cgi-bin/app.cgi",
			Path::new("/var/www"),
			&[],
			&[],
		));
		assert_eq!(lookup(&env, "QUERY_STRING"), Some(""));
		assert_eq!(lookup(&env, "REQUEST_URI"), Some("/cgi-bin/app.cgi"));
	}

	#[test]
	fn build_env_lifts_content_length_and_content_type() {
		let mut headers = http::HeaderMap::new();
		headers.insert(http::header::CONTENT_LENGTH, "42".parse().unwrap());
		headers.insert(http::header::CONTENT_TYPE, "application/json".parse().unwrap());
		let env = build_env(&meta("/script", None, &headers, "/script", Path::new("/wd"), &[], &[]));
		assert_eq!(lookup(&env, "CONTENT_LENGTH"), Some("42"));
		assert_eq!(lookup(&env, "CONTENT_TYPE"), Some("application/json"));
		// And NOT in the HTTP_* passthrough.
		assert!(lookup(&env, "HTTP_CONTENT_LENGTH").is_none());
		assert!(lookup(&env, "HTTP_CONTENT_TYPE").is_none());
	}

	#[test]
	fn build_env_passes_other_headers_through_http_namespace() {
		let mut headers = http::HeaderMap::new();
		headers.insert("user-agent", "curl/8".parse().unwrap());
		headers.insert("x-forwarded-for", "10.0.0.1".parse().unwrap());
		let env = build_env(&meta("/script", None, &headers, "/script", Path::new("/wd"), &[], &[]));
		assert_eq!(lookup(&env, "HTTP_USER_AGENT"), Some("curl/8"));
		assert_eq!(lookup(&env, "HTTP_X_FORWARDED_FOR"), Some("10.0.0.1"));
	}

	#[test]
	fn build_env_drops_blocked_headers_from_passthrough() {
		let mut headers = http::HeaderMap::new();
		headers.insert("authorization", "Bearer secret".parse().unwrap());
		headers.insert("user-agent", "curl/8".parse().unwrap());
		let block: Vec<String> = vec!["Authorization".into()];
		let env = build_env(&meta("/script", None, &headers, "/script", Path::new("/wd"), &block, &[]));
		assert!(lookup(&env, "HTTP_AUTHORIZATION").is_none());
		assert_eq!(lookup(&env, "HTTP_USER_AGENT"), Some("curl/8"));
	}

	#[test]
	fn build_env_uses_host_header_when_present() {
		let mut headers = http::HeaderMap::new();
		headers.insert(http::header::HOST, "example.com:8443".parse().unwrap());
		let env = build_env(&meta("/script", None, &headers, "/script", Path::new("/wd"), &[], &[]));
		assert_eq!(lookup(&env, "SERVER_NAME"), Some("example.com:8443"));
	}

	#[test]
	fn build_env_falls_back_to_server_ip_when_no_host_header() {
		let headers = empty_headers();
		let env = build_env(&meta("/script", None, &headers, "/script", Path::new("/wd"), &[], &[]));
		assert_eq!(lookup(&env, "SERVER_NAME"), Some("127.0.0.1"));
	}

	#[test]
	fn build_env_sets_https_when_tls() {
		let headers = empty_headers();
		let mut m = meta("/script", None, &headers, "/script", Path::new("/wd"), &[], &[]);
		m.is_tls = true;
		let env = build_env(&m);
		assert_eq!(lookup(&env, "REQUEST_SCHEME"), Some("https"));
		assert_eq!(lookup(&env, "HTTPS"), Some("on"));
	}

	#[test]
	fn build_env_omits_https_when_plaintext() {
		let headers = empty_headers();
		let env = build_env(&meta("/script", None, &headers, "/script", Path::new("/wd"), &[], &[]));
		assert_eq!(lookup(&env, "REQUEST_SCHEME"), Some("http"));
		assert!(lookup(&env, "HTTPS").is_none());
	}

	#[test]
	fn build_env_appends_extra_env_after_passthrough() {
		let headers = empty_headers();
		let extras = vec![("APP_DEBUG".to_owned(), "1".to_owned())];
		let env =
			build_env(&meta("/script", None, &headers, "/script", Path::new("/wd"), &[], &extras));
		assert_eq!(lookup(&env, "APP_DEBUG"), Some("1"));
	}

	#[test]
	fn build_env_carries_remote_port_and_remote_addr() {
		let headers = empty_headers();
		let env = build_env(&meta("/script", None, &headers, "/script", Path::new("/wd"), &[], &[]));
		assert_eq!(lookup(&env, "REMOTE_ADDR"), Some("203.0.113.7"));
		assert_eq!(lookup(&env, "REMOTE_HOST"), Some("203.0.113.7"));
		assert_eq!(lookup(&env, "REMOTE_PORT"), Some("54321"));
		assert_eq!(lookup(&env, "SERVER_PORT"), Some("8080"));
		assert_eq!(lookup(&env, "SERVER_SOFTWARE"), Some("test/1.0"));
	}
}
