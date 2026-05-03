use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::fetch::FetchKind;
use crate::predicate::Predicate;

pub type ListenSpec = String;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RawRule {
	pub name: String,
	pub listen: Vec<ListenSpec>,
	#[serde(default, rename = "match")]
	pub match_predicate: Option<Predicate>,
	#[serde(default)]
	pub middleware_chain: Vec<MiddlewareRef>,
	pub terminate: TerminateSpec,
	/// Optional TLS termination config. When set, the listener wraps
	/// each accepted TCP stream in a `rustls` server-side handshake
	/// before driving the L7 sub-graph; cleartext sockets get
	/// `Box<dyn AsyncReadWrite>` instead of raw `TcpStream`.
	///
	/// `lower_port` enforces consistency: every rule on the same
	/// listener must agree on `tls` (all `None` or all the same
	/// `Some(_)`); L4-only listeners cannot carry TLS (terminate +
	/// re-emit cleartext is not a useful proxy shape — it leaks the
	/// upstream traffic).
	#[serde(default)]
	pub tls: Option<TlsConfig>,
	/// Per-rule TLS 1.3 0-RTT (early data) acceptance. Required on
	/// every rule whose listener is TLS-terminating L7; absent on
	/// rules whose listener is plaintext or pure-L4 (a present value
	/// in those positions is a compile error). See
	/// `08-tls.md` § _TLS 1.3 0-RTT (early data)_.
	#[serde(default)]
	pub allow_zero_rtt: Option<bool>,
	/// Maximum bytes to buffer for request body `LazyBuffer` collection.
	/// Default 8 MiB. Exceeding this produces 413 Payload Too Large.
	#[serde(default = "default_max_body_bytes")]
	pub max_body_bytes_request: usize,
	/// Maximum bytes to buffer for response body `LazyBuffer` collection.
	/// Default 8 MiB. Exceeding this produces 502 Bad Gateway.
	#[serde(default = "default_max_body_bytes")]
	pub max_body_bytes_response: usize,
	#[serde(default)]
	pub source: SourceInfo,
}

fn default_max_body_bytes() -> usize {
	8 * 1024 * 1024
}

/// Listener-side TLS termination config — paths to the cert chain +
/// private key in PEM, plus an optional SNI hostname this cert serves.
///
/// `sni: None` marks the cert as the listener's _default_ — used when
/// the `ClientHello` has no SNI extension, or when the SNI doesn't
/// match any of the listener's `Some(_)` entries. A listener has at
/// most one default cert.
///
/// SNI hostnames are normalised to ASCII-lowercase at every ingest
/// boundary per 08-tls.md § _SNI normalization_; comparison against
/// rustls's already-lowercased `ClientHello::server_name()` is then
/// byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct TlsConfig {
	#[serde(default)]
	pub sni: Option<String>,
	pub cert_file: PathBuf,
	pub key_file: PathBuf,
	/// Listener-side TLS 1.3 0-RTT opt-in. Required on every rule that
	/// carries a `tls` block; rules sharing one listener must agree on
	/// this value (lower aggregates them). See
	/// `08-tls.md` § _TLS 1.3 0-RTT (early data)_.
	pub enable_zero_rtt: bool,
	/// Listener-side mTLS — per `08-tls.md` § _Client certificate
	/// verification_. Per-rule input; the lower pass aggregates each
	/// rule's `client_auth` into one `ClientAuthSpec` per listener
	/// address (rules on the same listener must agree, else compile
	/// error). `None` keeps the listener at `ClientAuth::None`.
	#[serde(default)]
	pub client_auth: Option<ClientAuthConfig>,
}

/// Per-rule mTLS config block, parsed from the `tls.client_auth` JSON.
/// `mode == None` is operator-explicit "don't request a cert"; the
/// trust store must be absent there. `mode == Request | Require`
/// requires a non-empty `trust_store`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ClientAuthConfig {
	pub mode: ClientAuthMode,
	#[serde(default)]
	pub trust_store: Option<ClientTrustStoreConfig>,
}

/// Three-valued client-auth mode (no implicit default per spec).
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientAuthMode {
	None,
	Request,
	Require,
}

/// Per-rule trust store config for verifying client certs. At least
/// one of `ca_paths` / `ca_dir` must be present (enforced at compile).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ClientTrustStoreConfig {
	#[serde(default)]
	pub ca_paths: Vec<PathBuf>,
	#[serde(default)]
	pub ca_dir: Option<PathBuf>,
	#[serde(default)]
	pub crls: Vec<CrlSourceConfig>,
}

/// One CRL source entry — file or URL, with a per-source
/// `fetch_failure` policy. URL sources are deferred (S3-11) and
/// rejected at compile time in this PR.
// TODO(s3-11): wire `Url` source kind, daemon-wide CRL cache,
// adaptive fetch cadence per `08-tls.md` § _CRL checking_.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CrlSourceConfig {
	File { path: PathBuf, fetch_failure: CrlFetchFailure },
	Url { url: String, fetch_failure: CrlFetchFailure },
}

/// CRL availability policy (per `08-tls.md` § _CRL checking_ § _Failure
/// handling_). Parsed eagerly though only the structural error path is
/// wired this PR — actual fetch / failure semantics land with S3-11.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CrlFetchFailure {
	Tolerate,
	Reject,
}

/// Per-listener cert pool — produced by `compile/lower` from every
/// rule on the bind address that carries a `tls` block, after
/// hash-consing identical entries and rejecting conflicts.
///
/// At most one `default` cert (sni-less); any number of SNI-keyed
/// certs. The engine's link stage compiles this into a single
/// `rustls::ServerConfig` whose cert resolver picks by SNI with
/// `default` as the fallback for unmatched / missing SNI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ListenerTlsSpec {
	#[serde(default)]
	pub default: Option<TlsConfig>,
	#[serde(default)]
	pub sni_certs: BTreeMap<String, TlsConfig>,
	/// Resolved per-listener mTLS policy. Per `08-tls.md` § _Client
	/// certificate verification_ this is per-listener, derived from the
	/// union of every rule's `tls.client_auth` on the same address;
	/// rules that disagree on `mode` or `trust_store` produce a compile
	/// error. Defaults to `None` for cleartext clients.
	#[serde(default)]
	pub client_auth: ClientAuthSpec,
	/// Resolved per-listener TLS 1.3 0-RTT opt-in. Aggregated by the
	/// lower pass from every rule's `tls.enable_zero_rtt` on the same
	/// address — rules that disagree produce a compile error. The
	/// engine's link wires this into `ServerConfig.max_early_data_size`
	/// (16 KiB when `true`, default 0 when `false`). Defaults to
	/// `false` for cleartext / non-TLS listeners. See
	/// `08-tls.md` § _TLS 1.3 0-RTT (early data)_.
	#[serde(default)]
	pub enable_zero_rtt: bool,
}

impl ListenerTlsSpec {
	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.default.is_none()
			&& self.sni_certs.is_empty()
			&& matches!(self.client_auth, ClientAuthSpec::None)
			&& !self.enable_zero_rtt
	}
}

/// Listener-level resolved mTLS policy. Built by the lower pass from
/// the union of per-rule `ClientAuthConfig` blocks; rules on the same
/// listener must all agree.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ClientAuthSpec {
	#[default]
	None,
	Request {
		trust_store: ClientTrustStoreConfig,
	},
	Require {
		trust_store: ClientTrustStoreConfig,
	},
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MiddlewareRef {
	#[serde(rename = "use")]
	pub name: String,
	#[serde(default)]
	pub args: Value,
	#[serde(default)]
	pub on_error: Option<OnErrorSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum OnErrorSpec {
	Close,
	Response(SynthResponse),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SynthResponse {
	pub status: u16,
	#[serde(default)]
	pub headers: Option<BTreeMap<String, String>>,
	#[serde(default)]
	pub body: Option<String>,
}

impl<'de> serde::Deserialize<'de> for OnErrorSpec {
	fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
		#[derive(serde::Deserialize)]
		#[serde(untagged)]
		enum Raw {
			Literal(String),
			Response { response: SynthResponse },
		}
		match Raw::deserialize(de)? {
			Raw::Literal(s) if s == "close" => Ok(Self::Close),
			Raw::Literal(other) => Err(serde::de::Error::unknown_variant(&other, &["close"])),
			Raw::Response { response } => Ok(Self::Response(response)),
		}
	}
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TerminateSpec {
	pub kind: FetchKind,
	pub args: Value,
}

impl<'de> serde::Deserialize<'de> for TerminateSpec {
	fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
		let mut v = Value::deserialize(de)?;
		let obj = v
			.as_object_mut()
			.ok_or_else(|| serde::de::Error::custom("`terminate` must be a JSON object"))?;
		let type_val = obj.remove("type").ok_or_else(|| serde::de::Error::missing_field("type"))?;
		let Value::String(alias) = type_val else {
			return Err(serde::de::Error::custom("`terminate.type` must be a string"));
		};
		let kind = fetch_kind_from_alias(&alias)
			.ok_or_else(|| serde::de::Error::custom(format!("unknown terminate type: {alias:?}")))?;
		// 05-terminator.md § _Variant ergonomics in config_:
		// `httpN_proxy` is sugar for `http_proxy` + `version: "hN"`.
		// Inject the version when the alias names a specific HTTP
		// version and the user has not already set one explicitly —
		// an explicit `args.version` always wins.
		if let Some(version) = http_version_from_alias(&alias)
			&& !obj.contains_key("version")
		{
			obj.insert("version".to_owned(), Value::String(version.to_owned()));
		}
		// `tcp_forward` / `udp_forward` are sugar for `L4Forward` +
		// `transport: "tcp" | "udp"`. Same precedence rule: an
		// explicit `args.transport` overrides the alias-derived value
		// (preserved as an escape hatch for hand-written rules).
		if let Some(transport) = transport_from_alias(&alias)
			&& !obj.contains_key("transport")
		{
			obj.insert("transport".to_owned(), Value::String(transport.to_owned()));
		}
		// Every `HttpProxy` alias resolves to one of the upstream kinds
		// the engine factory dispatches on: socket-based proxies
		// (`http_proxy` / `httpN_proxy` / `unix_proxy`) carry
		// `upstream_kind: "tcp"`; the CGI alias carries
		// `upstream_kind: "cgi"`. Injecting the marker explicitly
		// (rather than letting the factory infer from which fields are
		// present) gives the engine a clean, fail-loud branch — a
		// missing `upstream` on a socket-based rule produces "missing
		// args.upstream", not "unknown CGI shape". An explicit
		// `args.upstream_kind` always wins, same precedence rule as
		// `version` / `transport`.
		if let Some(upstream_kind) = upstream_kind_from_alias(&alias)
			&& !obj.contains_key("upstream_kind")
		{
			obj.insert("upstream_kind".to_owned(), Value::String(upstream_kind.to_owned()));
		}
		Ok(Self { kind, args: v })
	}
}

fn fetch_kind_from_alias(alias: &str) -> Option<FetchKind> {
	match alias {
		"tcp_forward" | "udp_forward" => Some(FetchKind::L4Forward),
		"http_proxy" | "http1_proxy" | "http2_proxy" | "http3_proxy" | "unix_proxy" | "cgi" => {
			Some(FetchKind::HttpProxy)
		}
		"websocket" => Some(FetchKind::WebSocketUpgrade),
		"static" | "redirect_https" => Some(FetchKind::HttpSynthesize),
		_ => None,
	}
}

fn http_version_from_alias(alias: &str) -> Option<&'static str> {
	match alias {
		"http1_proxy" => Some("h1"),
		"http2_proxy" => Some("h2"),
		"http3_proxy" => Some("h3"),
		_ => None,
	}
}

fn transport_from_alias(alias: &str) -> Option<&'static str> {
	match alias {
		"tcp_forward" => Some("tcp"),
		"udp_forward" => Some("udp"),
		_ => None,
	}
}

fn upstream_kind_from_alias(alias: &str) -> Option<&'static str> {
	match alias {
		"http_proxy" | "http1_proxy" | "http2_proxy" | "http3_proxy" | "unix_proxy" => Some("tcp"),
		"cgi" => Some("cgi"),
		_ => None,
	}
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct SourceInfo {
	#[serde(default)]
	pub file: PathBuf,
	#[serde(default)]
	pub line: u32,
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::predicate::{CheckMap, FieldPath, Operator, Predicate, Value as PredValue};

	#[test]
	fn raw_rule_minimal_parses_with_defaults() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse minimal rule");
		assert_eq!(rule.name, "r");
		assert_eq!(rule.listen, vec![":443".to_string()]);
		assert!(rule.match_predicate.is_none());
		assert!(rule.middleware_chain.is_empty());
		assert_eq!(rule.terminate.kind, FetchKind::HttpProxy);
		assert_eq!(
			rule.terminate.args,
			serde_json::json!({ "upstream": "127.0.0.1:8080", "upstream_kind": "tcp" }),
		);
		assert_eq!(rule.source.file, PathBuf::new());
		assert_eq!(rule.source.line, 0);
		assert_eq!(rule.max_body_bytes_request, 8 * 1024 * 1024);
		assert_eq!(rule.max_body_bytes_response, 8 * 1024 * 1024);
	}

	#[test]
	fn raw_rule_full_populates_every_field() {
		let raw = serde_json::json!({
			"name": "api",
			"listen": [":443", "0.0.0.0:80"],
			"match": { "tls.sni": { "equals": "api.example.com" } },
			"middleware_chain": [
				{ "use": "rate_limit", "args": { "rate": 100 } },
				{ "use": "jwt", "args": { "secret": "x" }, "on_error": "close" },
			],
			"terminate": {
				"type": "http_proxy",
				"upstream": "127.0.0.1:8080",
				"timeouts": { "connect": "5s" }
			},
			"source": { "file": "rules/30-api.json", "line": 14 },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse full rule");
		assert_eq!(rule.name, "api");
		assert_eq!(rule.listen.len(), 2);
		let check = match rule.match_predicate.as_ref().expect("match present") {
			Predicate::Check(c) => c,
			other => panic!("expected Check, got {other:?}"),
		};
		assert_eq!(check.path, FieldPath::TlsSni);
		match &check.op {
			Operator::Equals(PredValue::Str(s)) => assert_eq!(s, "api.example.com"),
			other => panic!("unexpected op: {other:?}"),
		}
		assert_eq!(rule.middleware_chain.len(), 2);
		assert_eq!(rule.middleware_chain[1].on_error, Some(OnErrorSpec::Close));
		assert_eq!(rule.terminate.kind, FetchKind::HttpProxy);
		assert_eq!(
			rule.terminate.args,
			serde_json::json!({
				"upstream": "127.0.0.1:8080",
				"upstream_kind": "tcp",
				"timeouts": { "connect": "5s" }
			}),
		);
		assert_eq!(rule.source.file, PathBuf::from("rules/30-api.json"));
		assert_eq!(rule.source.line, 14);
	}

	#[test]
	fn middleware_ref_flat_form_parses_name_and_args() {
		let raw = serde_json::json!({ "use": "rate_limit", "args": { "rate": 100 } });
		let m: MiddlewareRef = serde_json::from_value(raw).expect("parse middleware ref");
		assert_eq!(m.name, "rate_limit");
		assert_eq!(m.args, serde_json::json!({ "rate": 100 }));
		assert!(m.on_error.is_none());
	}

	#[test]
	fn middleware_ref_on_error_close_form() {
		let raw = serde_json::json!({ "use": "jwt", "args": { "secret": "x" }, "on_error": "close" });
		let m: MiddlewareRef = serde_json::from_value(raw).expect("parse middleware ref");
		assert_eq!(m.on_error, Some(OnErrorSpec::Close));
	}

	#[test]
	fn middleware_ref_on_error_response_object_form() {
		let raw = serde_json::json!({
			"use": "jwt",
			"on_error": { "response": { "status": 503, "body": "maintenance" } },
		});
		let m: MiddlewareRef = serde_json::from_value(raw).expect("parse middleware ref");
		assert_eq!(m.name, "jwt");
		assert_eq!(m.args, Value::Null);
		let resp = match m.on_error.expect("on_error present") {
			OnErrorSpec::Response(r) => r,
			OnErrorSpec::Close => panic!("expected Response"),
		};
		assert_eq!(resp.status, 503);
		assert_eq!(resp.body.as_deref(), Some("maintenance"));
		assert!(resp.headers.is_none());
	}

	#[test]
	fn middleware_ref_args_defaults_to_null_when_omitted() {
		let raw = serde_json::json!({ "use": "tag" });
		let m: MiddlewareRef = serde_json::from_value(raw).expect("parse middleware ref");
		assert_eq!(m.args, Value::Null);
	}

	#[test]
	fn middleware_ref_requires_use_key() {
		let raw = serde_json::json!({});
		let err = serde_json::from_value::<MiddlewareRef>(raw).expect_err("missing `use` must fail");
		let _ = err.to_string();
	}

	#[test]
	fn on_error_spec_string_invalid_variant_rejected() {
		let raw = serde_json::json!("crash");
		let err = serde_json::from_value::<OnErrorSpec>(raw).expect_err("non-`close` literal rejected");
		let msg = err.to_string();
		assert!(msg.contains("close"), "error names the only valid literal: {msg}");
	}

	#[test]
	fn on_error_spec_malformed_response_object_rejected() {
		let raw = serde_json::json!({ "response": null });
		let err = serde_json::from_value::<OnErrorSpec>(raw).expect_err("null response rejected");
		let _ = err.to_string();
	}

	#[test]
	fn on_error_spec_close_literal_parses() {
		let raw = serde_json::json!("close");
		let s: OnErrorSpec = serde_json::from_value(raw).expect("close literal parses");
		assert_eq!(s, OnErrorSpec::Close);
	}

	#[test]
	fn on_error_spec_response_object_parses() {
		let raw = serde_json::json!({
			"response": { "status": 503, "body": "maintenance" },
		});
		let s: OnErrorSpec = serde_json::from_value(raw).expect("response object parses");
		match s {
			OnErrorSpec::Response(r) => {
				assert_eq!(r.status, 503);
				assert_eq!(r.body.as_deref(), Some("maintenance"));
				assert!(r.headers.is_none());
			}
			OnErrorSpec::Close => panic!("expected Response"),
		}
	}

	#[test]
	fn synth_response_minimal_status_only() {
		let raw = serde_json::json!({ "status": 200 });
		let r: SynthResponse = serde_json::from_value(raw).expect("parse status-only synth");
		assert_eq!(r.status, 200);
		assert!(r.headers.is_none());
		assert!(r.body.is_none());
	}

	#[test]
	fn synth_response_full_status_headers_body() {
		let raw = serde_json::json!({
			"status": 404,
			"headers": { "content-type": "text/plain" },
			"body": "not found",
		});
		let r: SynthResponse = serde_json::from_value(raw).expect("parse full synth");
		assert_eq!(r.status, 404);
		let headers = r.headers.as_ref().expect("headers present");
		assert_eq!(headers.get("content-type").map(String::as_str), Some("text/plain"));
		assert_eq!(r.body.as_deref(), Some("not found"));
	}

	#[test]
	fn terminate_spec_alias_table_maps_to_fetch_kind() {
		// Every row of 05-terminator.md § _Variant ergonomics in config_.
		let cases: &[(&str, FetchKind)] = &[
			("tcp_forward", FetchKind::L4Forward),
			("udp_forward", FetchKind::L4Forward),
			("http_proxy", FetchKind::HttpProxy),
			("http1_proxy", FetchKind::HttpProxy),
			("http2_proxy", FetchKind::HttpProxy),
			("http3_proxy", FetchKind::HttpProxy),
			("unix_proxy", FetchKind::HttpProxy),
			("cgi", FetchKind::HttpProxy),
			("websocket", FetchKind::WebSocketUpgrade),
			("static", FetchKind::HttpSynthesize),
			("redirect_https", FetchKind::HttpSynthesize),
		];
		for (alias, expected) in cases {
			let raw = serde_json::json!({ "type": alias });
			let t: TerminateSpec =
				serde_json::from_value(raw).unwrap_or_else(|e| panic!("alias {alias} must parse: {e}"));
			assert_eq!(t.kind, *expected, "alias {alias} must map to {expected:?}");
		}
	}

	#[test]
	fn terminate_spec_args_preserves_all_non_type_keys_verbatim() {
		// 14-presets.md § _RawRule shape_: "every other key goes into `args`
		// verbatim". Covers top-level scalars AND nested objects.
		let raw = serde_json::json!({
			"type": "http_proxy",
			"upstream": "127.0.0.1:8080",
			"timeouts": { "connect": "5s", "total": "60s" },
		});
		let t: TerminateSpec = serde_json::from_value(raw).expect("parse");
		assert_eq!(t.kind, FetchKind::HttpProxy);
		assert_eq!(
			t.args,
			serde_json::json!({
				"upstream": "127.0.0.1:8080",
				"upstream_kind": "tcp",
				"timeouts": { "connect": "5s", "total": "60s" },
			}),
		);
	}

	#[test]
	fn terminate_spec_udp_forward_alias_injects_transport_udp() {
		let raw = serde_json::json!({ "type": "udp_forward", "upstream": "1.2.3.4:53" });
		let t: TerminateSpec = serde_json::from_value(raw).expect("parse");
		assert_eq!(t.kind, FetchKind::L4Forward);
		assert_eq!(t.args["transport"], "udp");
		assert_eq!(t.args["upstream"], "1.2.3.4:53");
	}

	#[test]
	fn terminate_spec_tcp_forward_alias_injects_transport_tcp() {
		let raw = serde_json::json!({ "type": "tcp_forward", "upstream": "10.0.0.5:22" });
		let t: TerminateSpec = serde_json::from_value(raw).expect("parse");
		assert_eq!(t.kind, FetchKind::L4Forward);
		assert_eq!(t.args["transport"], "tcp");
	}

	#[test]
	fn terminate_spec_cgi_alias_injects_upstream_kind_cgi() {
		// The factory branches on `args.upstream_kind`; the alias
		// resolution layer is what injects it. A bare `cgi` alias must
		// surface as `upstream_kind: "cgi"` so the engine factory can
		// dispatch without re-checking the alias.
		let raw = serde_json::json!({ "type": "cgi", "binary": "/usr/bin/true" });
		let t: TerminateSpec = serde_json::from_value(raw).expect("parse");
		assert_eq!(t.kind, FetchKind::HttpProxy);
		assert_eq!(t.args["upstream_kind"], "cgi");
	}

	#[test]
	fn terminate_spec_http_proxy_aliases_inject_upstream_kind_tcp() {
		// Every socket-based HttpProxy alias carries
		// `upstream_kind: "tcp"`. Explicit injection (rather than
		// leaving the marker absent for socket variants) makes the
		// factory's dispatch table closed — no implicit fallback.
		for alias in ["http_proxy", "http1_proxy", "http2_proxy", "http3_proxy", "unix_proxy"] {
			let raw = serde_json::json!({ "type": alias, "upstream": "127.0.0.1:8080" });
			let t: TerminateSpec =
				serde_json::from_value(raw).unwrap_or_else(|e| panic!("alias {alias} must parse: {e}"));
			assert_eq!(t.args["upstream_kind"], "tcp", "alias {alias} must inject upstream_kind: tcp");
		}
	}

	#[test]
	fn terminate_spec_explicit_upstream_kind_wins_over_alias() {
		// Same escape-hatch rule the version/transport injections
		// follow: an operator-supplied `args.upstream_kind` is never
		// overridden by the alias-derived value.
		let raw = serde_json::json!({
			"type": "http_proxy",
			"upstream": "127.0.0.1:8080",
			"upstream_kind": "tcp",
		});
		let t: TerminateSpec = serde_json::from_value(raw).expect("parse");
		assert_eq!(t.args["upstream_kind"], "tcp");
	}

	#[test]
	fn terminate_spec_explicit_transport_wins_over_alias() {
		// Explicit `args.transport` always overrides the alias-derived
		// value — escape hatch for hand-written configs that want to
		// pin a transport regardless of which alias spelled the rule.
		let raw = serde_json::json!({ "type": "udp_forward", "upstream": "x", "transport": "tcp" });
		let t: TerminateSpec = serde_json::from_value(raw).expect("parse");
		assert_eq!(t.args["transport"], "tcp");
	}

	#[test]
	fn terminate_spec_alias_only_yields_object_with_injected_markers() {
		// 14-presets.md § _RawRule shape_: the custom Deserialize removes `type`
		// from a JSON object and keeps the rest. An alias-only terminate keeps
		// the object shape; it now also carries the alias-resolution markers
		// (`upstream_kind` for `HttpProxy` aliases). The point of this test is
		// to lock in "args is an object, not Value::Null" — which the marker
		// injection only reinforces.
		let raw = serde_json::json!({ "type": "http_proxy" });
		let t: TerminateSpec = serde_json::from_value(raw).expect("parse");
		assert_eq!(t.kind, FetchKind::HttpProxy);
		assert!(t.args.is_object(), "args must be an object, got {:?}", t.args);
		assert_eq!(t.args["upstream_kind"], "tcp");
	}

	#[test]
	fn terminate_spec_unknown_type_rejected_and_names_alias() {
		let raw = serde_json::json!({ "type": "bogus" });
		let err = serde_json::from_value::<TerminateSpec>(raw).expect_err("unknown alias rejected");
		assert!(err.to_string().contains("bogus"), "error must name the offending alias: {err}");
	}

	#[test]
	fn terminate_spec_missing_type_rejected_and_names_field() {
		let raw = serde_json::json!({ "upstream": "127.0.0.1:8080" });
		let err = serde_json::from_value::<TerminateSpec>(raw).expect_err("missing type rejected");
		assert!(err.to_string().contains("type"), "error must name the missing field: {err}");
	}

	#[test]
	fn source_info_default_is_empty_path_and_zero_line() {
		let s = SourceInfo::default();
		assert_eq!(s.file, PathBuf::new());
		assert_eq!(s.line, 0);
	}

	#[test]
	fn source_info_round_trip_via_json() {
		let raw = serde_json::json!({ "file": "rules/a.json", "line": 7 });
		let s: SourceInfo = serde_json::from_value(raw).expect("parse source info");
		assert_eq!(s.file, PathBuf::from("rules/a.json"));
		assert_eq!(s.line, 7);
	}

	#[test]
	fn middleware_chain_defaults_to_empty_when_omitted() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"terminate": { "type": "http_proxy" },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse");
		assert!(rule.middleware_chain.is_empty());
	}

	#[test]
	fn middleware_ref_chain_mixes_on_error_forms() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"middleware_chain": [
				{ "use": "a" },
				{ "use": "b", "on_error": "close" },
				{ "use": "c", "on_error": { "response": { "status": 500 } } },
			],
			"terminate": { "type": "http_proxy" },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse");
		assert_eq!(rule.middleware_chain.len(), 3);
		assert!(rule.middleware_chain[0].on_error.is_none());
		assert_eq!(rule.middleware_chain[1].on_error, Some(OnErrorSpec::Close));
		match rule.middleware_chain[2].on_error.as_ref().expect("on_error[2]") {
			OnErrorSpec::Response(r) => {
				assert_eq!(r.status, 500);
				assert!(r.body.is_none());
				assert!(r.headers.is_none());
			}
			OnErrorSpec::Close => panic!("expected Response at index 2"),
		}
	}

	#[test]
	fn raw_rule_accepts_top_level_check_predicate() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":80"],
			"match": { "http.uri.path": { "prefix": "/api" } },
			"terminate": { "type": "http_proxy" },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse");
		let Some(Predicate::Check(CheckMap { path, op })) = rule.match_predicate else {
			panic!("expected Check predicate");
		};
		assert_eq!(path, FieldPath::HttpUriPath);
		match op {
			Operator::Prefix(PredValue::Str(s)) => assert_eq!(s, "/api"),
			other => panic!("unexpected op: {other:?}"),
		}
	}

	#[test]
	fn raw_rule_without_tls_field_defaults_to_none() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":80"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse rule without tls");
		assert!(rule.tls.is_none());
	}

	#[test]
	fn raw_rule_with_tls_field_parses_paths() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
			"tls": {
				"cert_file": "/etc/vaned/certs/api.pem",
				"key_file": "/etc/vaned/certs/api.key",
				"enable_zero_rtt": false,
			},
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse rule with tls");
		let tls = rule.tls.expect("tls present");
		assert_eq!(tls.cert_file, PathBuf::from("/etc/vaned/certs/api.pem"));
		assert_eq!(tls.key_file, PathBuf::from("/etc/vaned/certs/api.key"));
		assert!(!tls.enable_zero_rtt);
	}

	#[test]
	fn tls_config_round_trips_through_json() {
		let original = TlsConfig {
			sni: None,
			cert_file: PathBuf::from("/srv/cert.pem"),
			key_file: PathBuf::from("/srv/key.pem"),
			enable_zero_rtt: false,
			client_auth: None,
		};
		let encoded = serde_json::to_string(&original).expect("serialize");
		let decoded: TlsConfig = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, original);
	}

	#[test]
	fn tls_config_with_sni_field_parses() {
		let raw = serde_json::json!({
			"sni": "api.example.com",
			"cert_file": "/etc/vaned/certs/api.pem",
			"key_file": "/etc/vaned/certs/api.key",
			"enable_zero_rtt": false,
		});
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse tls with sni");
		assert_eq!(tls.sni.as_deref(), Some("api.example.com"));
	}

	#[test]
	fn tls_config_without_sni_parses_with_none() {
		let raw = serde_json::json!({
			"cert_file": "/etc/vaned/certs/default.pem",
			"key_file": "/etc/vaned/certs/default.key",
			"enable_zero_rtt": false,
		});
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse tls without sni");
		assert!(tls.sni.is_none());
	}

	#[test]
	fn tls_config_missing_enable_zero_rtt_field_rejected() {
		// `enable_zero_rtt` is required (no implicit default) per
		// `08-tls.md` § _TLS 1.3 0-RTT_; absence on a `tls` block is a
		// hard parse error before the lower pass even sees the rule.
		let raw = serde_json::json!({
			"cert_file": "/etc/vaned/certs/default.pem",
			"key_file": "/etc/vaned/certs/default.key",
		});
		let err =
			serde_json::from_value::<TlsConfig>(raw).expect_err("missing enable_zero_rtt must reject");
		assert!(
			err.to_string().contains("enable_zero_rtt"),
			"error must name the missing field: {err}",
		);
	}

	#[test]
	fn raw_rule_allow_zero_rtt_field_parses_when_present() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
			"allow_zero_rtt": true,
			"tls": {
				"cert_file": "/etc/vaned/certs/api.pem",
				"key_file": "/etc/vaned/certs/api.key",
				"enable_zero_rtt": true,
			},
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse rule with allow_zero_rtt");
		assert_eq!(rule.allow_zero_rtt, Some(true));
	}

	#[test]
	fn raw_rule_allow_zero_rtt_defaults_to_none_when_omitted() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":80"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse rule without allow_zero_rtt");
		assert!(rule.allow_zero_rtt.is_none());
	}
}
