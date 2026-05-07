//! `redirect_https` preset — HTTP→HTTPS redirect via 308.
//!
//! Expands to a single `RawRule` whose terminate is `HttpSynthesize`
//! emitting a 308 with `Location: https://${host}${uri}`. See
//! `spec/crates/core.md` § _`redirect_https`_.
//!
//! SPEC DEVIATION (carried as a known debt): the spec implies runtime
//! template substitution (`${host}` / `${uri}` resolve to the request's
//! Host header and URI). `HttpSynthesizeFetch` currently has no template
//! engine — the literal string `https://${host}${uri}` is what reaches
//! the client. A follow-up wires substitution; until then this preset
//! is functional for diagnostics but not for production redirects. Flag
//! preserved in commit body, not as a runtime-time error.

use crate::error::Error;
use crate::fetch::FetchKind;
use crate::preset::PresetInvocation;
use crate::rule::{RawRule, TerminateSpec};

pub(super) fn expand(inv: PresetInvocation) -> Result<Vec<RawRule>, Error> {
	if !inv.args.is_null() && !inv.args.as_object().is_some_and(serde_json::Map::is_empty) {
		return Err(Error::compile(format!("preset redirect_https takes no args; got {}", inv.args)));
	}

	let terminate_args = serde_json::json!({
		"status": 308,
		"headers": { "location": "https://${host}${uri}" },
	});

	// Presets emit `allow_zero_rtt` explicitly per `spec/crates/engine-tls.md` § _TLS
	// 1.3 0-RTT_'s "CLI / TUI emits `false` when 0-RTT is not in use".
	// `Some(false)` mirrors the operator-default posture; rules whose
	// listener is plaintext propagate `None` so the lower pass does not
	// flag a misplaced field.
	let allow_zero_rtt = inv.tls.as_ref().map(|_| false);
	Ok(vec![RawRule {
		name: inv.name,
		listen: inv.listen,
		match_predicate: None,
		middleware_chain: vec![],
		terminate: TerminateSpec { kind: FetchKind::HttpSynthesize, args: terminate_args },
		tls: inv.tls,
		allow_zero_rtt,
		max_body_bytes_request: 8 * 1024 * 1024,
		max_body_bytes_response: 8 * 1024 * 1024,
		source: inv.source,
	}])
}

#[cfg(test)]
mod tests {
	use serde_json::Value;

	use super::*;
	use crate::rule::SourceInfo;

	fn invoke(args: Value) -> PresetInvocation {
		PresetInvocation {
			name: "http".to_string(),
			preset: "redirect_https".to_string(),
			listen: vec![":80".into()],
			args,
			tls: None,
			source: SourceInfo::default(),
		}
	}

	#[test]
	fn redirect_https_emits_308_with_host_uri_template() {
		let rules = expand(invoke(Value::Null)).expect("expand");
		assert_eq!(rules.len(), 1);
		let r = &rules[0];
		assert_eq!(r.terminate.kind, FetchKind::HttpSynthesize);
		assert_eq!(r.terminate.args["status"], 308);
		assert_eq!(r.terminate.args["headers"]["location"], "https://${host}${uri}");
		assert!(r.middleware_chain.is_empty());
	}

	#[test]
	fn redirect_https_takes_no_args() {
		let err = expand(invoke(serde_json::json!({ "status": 301 }))).expect_err("args rejected");
		assert!(err.to_string().contains("no args"), "error explains the constraint: {err}");
	}

	#[test]
	fn redirect_https_accepts_empty_object_as_no_args() {
		let rules = expand(invoke(serde_json::json!({}))).expect("empty object equivalent to null");
		assert_eq!(rules.len(), 1);
	}

	#[test]
	fn redirect_https_preserves_listen() {
		let rules = expand(invoke(Value::Null)).expect("expand");
		assert_eq!(rules[0].listen, vec![":80".to_string()]);
		assert_eq!(rules[0].name, "http");
	}
}
