use std::collections::BTreeMap;
use std::path::PathBuf;

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
	#[serde(default)]
	pub fetch: Option<FetchSpec>,
	pub terminate: TerminatorSpec,
	#[serde(default)]
	pub source: SourceInfo,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MiddlewareRef {
	#[serde(rename = "use")]
	pub name: String,
	#[serde(default)]
	pub args: serde_json::Value,
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct FetchSpec {
	#[serde(rename = "type")]
	pub kind: String,
	#[serde(default)]
	pub args: serde_json::Value,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TerminatorSpec {
	#[serde(rename = "type")]
	pub kind: String,
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
	use crate::predicate::{CheckMap, FieldPath, Operator, Predicate, Value};

	#[test]
	fn raw_rule_minimal_parses_with_defaults() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"terminate": { "type": "http_proxy" },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse minimal rule");
		assert_eq!(rule.name, "r");
		assert_eq!(rule.listen, vec![":443".to_string()]);
		assert!(rule.match_predicate.is_none());
		assert!(rule.middleware_chain.is_empty());
		assert!(rule.fetch.is_none());
		assert_eq!(rule.terminate.kind, "http_proxy");
		assert_eq!(rule.source.file, PathBuf::new());
		assert_eq!(rule.source.line, 0);
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
			"fetch": { "type": "http_proxy", "args": { "upstream": "127.0.0.1:8080" } },
			"terminate": { "type": "write_http_response" },
			"source": { "file": "rules/30-api.json", "line": 14 },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse full rule");
		assert_eq!(rule.name, "api");
		assert_eq!(rule.listen.len(), 2);
		assert_eq!(rule.listen[0], ":443");
		assert_eq!(rule.listen[1], "0.0.0.0:80");
		let check = match rule.match_predicate.as_ref().expect("match present") {
			Predicate::Check(c) => c,
			other => panic!("expected Check, got {other:?}"),
		};
		assert_eq!(check.path, FieldPath::TlsSni);
		match &check.op {
			Operator::Equals(Value::Str(s)) => assert_eq!(s, "api.example.com"),
			other => panic!("unexpected op: {other:?}"),
		}
		assert_eq!(rule.middleware_chain.len(), 2);
		assert_eq!(rule.middleware_chain[0].name, "rate_limit");
		assert_eq!(rule.middleware_chain[0].args, serde_json::json!({ "rate": 100 }));
		assert!(rule.middleware_chain[0].on_error.is_none());
		assert_eq!(rule.middleware_chain[1].name, "jwt");
		assert_eq!(rule.middleware_chain[1].args, serde_json::json!({ "secret": "x" }));
		assert_eq!(rule.middleware_chain[1].on_error, Some(OnErrorSpec::Close));
		let fetch = rule.fetch.as_ref().expect("fetch present");
		assert_eq!(fetch.kind, "http_proxy");
		assert_eq!(fetch.args, serde_json::json!({ "upstream": "127.0.0.1:8080" }));
		assert_eq!(rule.terminate.kind, "write_http_response");
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
		// args omitted → default to Value::Null
		assert_eq!(m.args, serde_json::Value::Null);
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
		assert_eq!(m.args, serde_json::Value::Null);
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
	fn fetch_spec_rename_and_args() {
		let raw = serde_json::json!({
			"type": "http_proxy",
			"args": { "upstream": "127.0.0.1:8080" },
		});
		let f: FetchSpec = serde_json::from_value(raw).expect("parse fetch");
		assert_eq!(f.kind, "http_proxy");
		assert_eq!(f.args, serde_json::json!({ "upstream": "127.0.0.1:8080" }));
	}

	#[test]
	fn fetch_spec_args_default_to_null() {
		let raw = serde_json::json!({ "type": "http_proxy" });
		let f: FetchSpec = serde_json::from_value(raw).expect("parse fetch with no args");
		assert_eq!(f.kind, "http_proxy");
		assert_eq!(f.args, serde_json::Value::Null);
	}

	#[test]
	fn terminator_spec_rename_kind() {
		let raw = serde_json::json!({ "type": "write_http_response" });
		let t: TerminatorSpec = serde_json::from_value(raw).expect("parse terminator");
		assert_eq!(t.kind, "write_http_response");
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
		// RawRule's middleware_chain carries #[serde(default)]; omitting the key
		// must produce an empty Vec rather than a parse error.
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
			"terminate": { "type": "write_http_response" },
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
		// Sanity: the RawRule#match key dispatches to Predicate's custom
		// Deserialize; a single-key check object must round-trip through it.
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":80"],
			"match": { "http.uri.path": { "prefix": "/api" } },
			"terminate": { "type": "write_http_response" },
		});
		let rule: RawRule = serde_json::from_value(raw).expect("parse");
		let Some(Predicate::Check(CheckMap { path, op })) = rule.match_predicate else {
			panic!("expected Check predicate");
		};
		assert_eq!(path, FieldPath::HttpUriPath);
		match op {
			Operator::Prefix(Value::Str(s)) => assert_eq!(s, "/api"),
			other => panic!("unexpected op: {other:?}"),
		}
	}
}
