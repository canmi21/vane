//! `static_site` preset — synthesise a fixed HTTP response.
//!
//! Expands to a single `RawRule` whose terminate is `HttpSynthesize`.
//! The user's `body` (plain UTF-8) is base64-encoded into the synth
//! fetch's `body` arg, matching the wire contract documented in
//! `crates/engine/src/fetch/http_synthesize.rs` (`body` is base64
//! because JSON has no native byte type).
//!
//! See `spec/crates/core.md` § _Compile pipeline_.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Map, Value};

use crate::error::Error;
use crate::fetch::FetchKind;
use crate::preset::PresetInvocation;
use crate::rule::{RawRule, TerminateSpec};

#[derive(serde::Deserialize)]
struct Args {
	status: u16,
	#[serde(default)]
	headers: Option<BTreeMap<String, String>>,
	#[serde(default)]
	body: Option<String>,
}

pub(super) fn expand(inv: PresetInvocation) -> Result<Vec<RawRule>, Error> {
	let args: Args = serde_json::from_value(inv.args.clone())
		.map_err(|e| Error::compile(format!("preset static_site args: {e}")))?;

	let mut terminate_args = Map::new();
	terminate_args.insert("status".to_string(), Value::Number(args.status.into()));
	if let Some(headers) = args.headers {
		// `BTreeMap<String, String>` round-trips through `serde_json::to_value`
		// without panic — the value type is plain JSON-friendly.
		let v = serde_json::to_value(headers)
			.map_err(|e| Error::compile(format!("preset static_site headers: {e}")))?;
		terminate_args.insert("headers".to_string(), v);
	}
	if let Some(body) = args.body {
		let encoded = BASE64_STANDARD.encode(body.as_bytes());
		terminate_args.insert("body".to_string(), Value::String(encoded));
	}

	let allow_zero_rtt = inv.tls.as_ref().map(|_| false);
	Ok(vec![RawRule {
		name: inv.name,
		listen: inv.listen,
		match_predicate: None,
		middleware_chain: vec![],
		terminate: TerminateSpec {
			kind: FetchKind::HttpSynthesize,
			args: Value::Object(terminate_args),
		},
		tls: inv.tls,
		allow_zero_rtt,
		max_body_bytes_request: 8 * 1024 * 1024,
		max_body_bytes_response: 8 * 1024 * 1024,
		source: inv.source,
	}])
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::rule::SourceInfo;

	fn invoke(args: Value) -> PresetInvocation {
		PresetInvocation {
			name: "site".to_string(),
			preset: "static_site".to_string(),
			listen: vec![":443".into()],
			args,
			tls: None,
			source: SourceInfo::default(),
		}
	}

	#[test]
	fn static_site_expands_with_status_only() {
		let rules = expand(invoke(serde_json::json!({ "status": 204 }))).expect("expand");
		assert_eq!(rules.len(), 1);
		assert_eq!(rules[0].terminate.kind, FetchKind::HttpSynthesize);
		assert_eq!(rules[0].terminate.args, serde_json::json!({ "status": 204 }));
	}

	#[test]
	fn static_site_body_is_base64_encoded_in_terminate_args() {
		let rules = expand(invoke(serde_json::json!({ "status": 200, "body": "Hello, world!" })))
			.expect("expand");
		let body = rules[0].terminate.args.get("body").and_then(Value::as_str).expect("body field");
		// Base64 of "Hello, world!" is SGVsbG8sIHdvcmxkIQ==
		assert_eq!(body, "SGVsbG8sIHdvcmxkIQ==");
	}

	#[test]
	fn static_site_headers_pass_through_verbatim() {
		let rules = expand(invoke(serde_json::json!({
			"status": 200,
			"headers": { "content-type": "text/plain", "x-via": "vane" }
		})))
		.expect("expand");
		let headers = rules[0].terminate.args.get("headers").expect("headers field");
		assert_eq!(headers["content-type"], "text/plain");
		assert_eq!(headers["x-via"], "vane");
	}

	#[test]
	fn static_site_rejects_missing_status() {
		let err = expand(invoke(serde_json::json!({ "body": "hi" }))).expect_err("missing status");
		assert!(err.to_string().contains("status"), "error names the missing field: {err}");
	}
}
