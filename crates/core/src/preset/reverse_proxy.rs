//! `reverse_proxy` preset — HTTP reverse proxy with sensible defaults.
//!
//! Expands a `PresetInvocation` into one or more `RawRule`s:
//!
//! - `<name>.main` — the `http_proxy` rule, optionally with
//!   `forward_client_ip` and `rate_limit` middleware in chain order.
//! - `<name>.ws` — emitted when `websocket: false`. Matches the
//!   `Upgrade: websocket` header and synthesises a 400. (The WS-disable
//!   gate; one rule.)
//! - `<name>.ws-allow` + `<name>.ws-deny` — emitted when `websocket` is
//!   a path-prefix array. The allow rule matches WS upgrade + path
//!   prefix and routes to `WebSocketUpgrade`; the deny rule matches WS
//!   upgrade alone and synthesises 400. Specificity-sort runs allow
//!   before deny so matching paths take the WS route and the rest fall
//!   through to rejection.
//! - `<name>.ws` (allow-all) — emitted when `websocket: true`. Matches
//!   WS upgrade and routes to `WebSocketUpgrade(upstream)`.
//!
//! See `spec/crates/core.md` § _Compile pipeline_ and
//! `spec/crates/engine.md` `spec/crates/engine.md` § _Concrete fetches_.
//!
//! ## Spec deviations (carried as known debts)
//!
// TODO(preset-default-rate-limit): the spec calls for built-in
// defaults (`rate=100/burst=200`) on the `reverse_proxy` preset. The
// rate-limit middleware is registered, but the preset still only
// emits a `rate_limit` ref when `args.rate_limit` is explicitly
// provided. Re-enable the defaults — and update the test that asserts
// "no implicit rate_limit" — when the contract solidifies.
//!
//! **Predicate shorthand.** Spec uses `[upgrade == websocket]` /
//! `[path.prefix in [...]]` shorthand; we emit the field-path form
//! (`http.header.upgrade equals "websocket"` and `http.uri.path prefix
//! p_i` inside an `any_of`). The shorthand is documentation, not a
//! literal grammar requirement — the field-path form is the canonical
//! wire shape.

use serde::Deserialize;
use serde_json::Value;

use crate::error::Error;
use crate::fetch::FetchKind;
use crate::preset::PresetInvocation;
use crate::rule::{ListenSpec, MiddlewareRef, RawRule, SourceInfo, TerminateSpec, TlsConfig};

#[derive(Deserialize)]
struct Args {
	upstream: String,

	#[serde(default)]
	websocket: WebSocketArg,

	#[serde(default = "default_true")]
	forward_client_ip: bool,

	#[serde(default)]
	rate_limit: Option<RateLimitArgs>,

	#[serde(default)]
	timeouts: Option<Value>,
}

const fn default_true() -> bool {
	true
}

#[derive(Default)]
enum WebSocketArg {
	#[default]
	Disabled,
	AllowAll,
	Paths(Vec<String>),
}

// Hand-written so:
//   `false` / missing             → Disabled
//   `true` / `"*"`                → AllowAll
//   ["/p", ...] (non-empty)       → Paths
// Anything else surfaces a pointed serde error.
impl<'de> Deserialize<'de> for WebSocketArg {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		let v = Value::deserialize(d)?;
		match v {
			Value::Bool(true) => Ok(Self::AllowAll),
			Value::String(s) if s == "*" => Ok(Self::AllowAll),
			Value::String(other) => Err(serde::de::Error::custom(format!(
				"websocket: expected bool, \"*\", or [path...] array; got {other:?}"
			))),
			Value::Array(arr) => {
				if arr.is_empty() {
					return Err(serde::de::Error::custom(
						"websocket array must contain at least one path prefix",
					));
				}
				let paths: Result<Vec<String>, _> = arr
					.into_iter()
					.map(|item| {
						item
							.as_str()
							.map(str::to_string)
							.ok_or_else(|| serde::de::Error::custom("websocket array items must be strings"))
					})
					.collect();
				paths.map(Self::Paths)
			}
			Value::Bool(false) | Value::Null => Ok(Self::Disabled),
			other => Err(serde::de::Error::custom(format!("websocket: unsupported shape {other}"))),
		}
	}
}

#[derive(Deserialize)]
struct RateLimitArgs {
	rate: u64,
	burst: u64,
	#[serde(default = "default_window")]
	window: String,
}

fn default_window() -> String {
	"1s".to_string()
}

pub(super) fn expand(inv: PresetInvocation) -> Result<Vec<RawRule>, Error> {
	let args: Args = serde_json::from_value(inv.args.clone())
		.map_err(|e| Error::compile(format!("preset reverse_proxy args: {e}")))?;

	let mut rules: Vec<RawRule> = Vec::new();

	// WebSocket gate rules. `spec/crates/engine.md` `spec/crates/engine.md` § _Concrete fetches_ — sort order
	// (ws-allow < ws-deny < main) is enforced by the analyze pass's
	// specificity ranking, not by emission order; ordering here is for
	// dry-run readability only.
	match &args.websocket {
		WebSocketArg::Disabled => {
			rules.push(ws_reject_rule(
				&format!("{}.ws", inv.name),
				&inv.listen,
				&inv.source,
				inv.tls.clone(),
			));
		}
		WebSocketArg::AllowAll => {
			rules.push(ws_passthrough_rule(
				&format!("{}.ws", inv.name),
				&inv.listen,
				&inv.source,
				&args.upstream,
				None,
				inv.tls.clone(),
			));
		}
		WebSocketArg::Paths(paths) => {
			rules.push(ws_passthrough_rule(
				&format!("{}.ws-allow", inv.name),
				&inv.listen,
				&inv.source,
				&args.upstream,
				Some(paths.clone()),
				inv.tls.clone(),
			));
			rules.push(ws_reject_rule(
				&format!("{}.ws-deny", inv.name),
				&inv.listen,
				&inv.source,
				inv.tls.clone(),
			));
		}
	}

	// Main http_proxy rule (always last in emission so dry-run reads
	// gate-then-main).
	let mut chain: Vec<MiddlewareRef> = Vec::new();
	if let Some(rl) = args.rate_limit.as_ref() {
		chain.push(MiddlewareRef {
			name: "rate_limit".to_string(),
			args: serde_json::json!({
				"rate": rl.rate,
				"burst": rl.burst,
				"window": rl.window,
			}),
			on_error: None,
		});
	}
	if args.forward_client_ip {
		chain.push(MiddlewareRef {
			name: "forward_client_ip".to_string(),
			args: Value::Null,
			on_error: None,
		});
	}

	let mut http_proxy_args =
		serde_json::Map::from_iter([("upstream".to_string(), Value::String(args.upstream.clone()))]);
	if let Some(t) = args.timeouts {
		http_proxy_args.insert("timeouts".to_string(), t);
	}

	let allow_zero_rtt_main = inv.tls.as_ref().map(|_| false);
	rules.push(RawRule {
		name: format!("{}.main", inv.name),
		listen: inv.listen,
		match_predicate: None,
		middleware_chain: chain,
		terminate: TerminateSpec { kind: FetchKind::HttpProxy, args: Value::Object(http_proxy_args) },
		tls: inv.tls,
		allow_zero_rtt: allow_zero_rtt_main,
		max_body_bytes_request: 8 * 1024 * 1024,
		max_body_bytes_response: 8 * 1024 * 1024,
		source: inv.source,
	});

	Ok(rules)
}

fn ws_upgrade_predicate() -> Value {
	serde_json::json!({ "http.header.upgrade": { "equals": "websocket" } })
}

fn ws_reject_rule(
	name: &str,
	listen: &[ListenSpec],
	source: &SourceInfo,
	tls: Option<TlsConfig>,
) -> RawRule {
	let predicate = serde_json::from_value(ws_upgrade_predicate())
		.expect("upgrade predicate is a hand-built valid CheckMap");
	let allow_zero_rtt = tls.as_ref().map(|_| false);
	RawRule {
		name: name.to_string(),
		listen: listen.to_vec(),
		match_predicate: Some(predicate),
		middleware_chain: vec![],
		terminate: TerminateSpec {
			kind: FetchKind::HttpSynthesize,
			args: serde_json::json!({ "status": 400 }),
		},
		tls,
		allow_zero_rtt,
		max_body_bytes_request: 8 * 1024 * 1024,
		max_body_bytes_response: 8 * 1024 * 1024,
		source: source.clone(),
	}
}

fn ws_passthrough_rule(
	name: &str,
	listen: &[ListenSpec],
	source: &SourceInfo,
	upstream: &str,
	paths: Option<Vec<String>>,
	tls: Option<TlsConfig>,
) -> RawRule {
	let predicate_value = match paths {
		Some(prefixes) => {
			// all_of [ upgrade == "websocket", any_of [ path.prefix = p_i ... ] ].
			// Both leaves sit at the L7Header level so the cross-level
			// validator accepts the combinator.
			let prefix_branches: Vec<serde_json::Value> = prefixes
				.into_iter()
				.map(|p| serde_json::json!({ "http.uri.path": { "prefix": p } }))
				.collect();
			serde_json::json!({
				"all_of": [
					{ "http.header.upgrade": { "equals": "websocket" } },
					{ "any_of": prefix_branches },
				],
			})
		}
		None => ws_upgrade_predicate(),
	};
	let predicate = serde_json::from_value(predicate_value)
		.expect("upgrade predicate is a hand-built valid CheckMap or AllOf");
	let allow_zero_rtt = tls.as_ref().map(|_| false);
	RawRule {
		name: name.to_string(),
		listen: listen.to_vec(),
		match_predicate: Some(predicate),
		middleware_chain: vec![],
		terminate: TerminateSpec {
			kind: FetchKind::WebSocketUpgrade,
			args: serde_json::json!({ "upstream": upstream }),
		},
		tls,
		allow_zero_rtt,
		max_body_bytes_request: 8 * 1024 * 1024,
		max_body_bytes_response: 8 * 1024 * 1024,
		source: source.clone(),
	}
}

#[cfg(test)]
#[allow(
	clippy::case_sensitive_file_extension_comparisons,
	reason = "rule name suffix, not a filesystem path"
)]
mod tests {
	use super::*;
	use crate::rule::SourceInfo;

	fn invoke(args: Value) -> PresetInvocation {
		PresetInvocation {
			name: "api".to_string(),
			preset: "reverse_proxy".to_string(),
			listen: vec![":443".into()],
			args,
			tls: None,
			source: SourceInfo::default(),
		}
	}

	fn rule_names(rules: &[RawRule]) -> Vec<&str> {
		rules.iter().map(|r| r.name.as_str()).collect()
	}

	fn find_main(rules: &[RawRule]) -> &RawRule {
		// `.main` is a name suffix, not a file extension — the lint's heuristic
		// triggers on every dot-bearing literal.
		rules.iter().find(|r| r.name.ends_with(".main")).expect("main rule present")
	}

	#[test]
	fn reverse_proxy_websocket_false_emits_ws_reject_and_main() {
		let rules =
			expand(invoke(serde_json::json!({ "upstream": "127.0.0.1:8080", "websocket": false })))
				.expect("expand");
		assert_eq!(rule_names(&rules), vec!["api.ws", "api.main"]);
		let ws = &rules[0];
		assert_eq!(ws.terminate.kind, FetchKind::HttpSynthesize);
		assert_eq!(ws.terminate.args["status"], 400);
		assert!(ws.match_predicate.is_some(), "ws-reject must carry the upgrade predicate");
	}

	#[test]
	fn reverse_proxy_websocket_true_emits_ws_passthrough_and_main() {
		let rules =
			expand(invoke(serde_json::json!({ "upstream": "u:1", "websocket": true }))).expect("expand");
		assert_eq!(rule_names(&rules), vec!["api.ws", "api.main"]);
		let ws = &rules[0];
		assert_eq!(ws.terminate.kind, FetchKind::WebSocketUpgrade);
		assert_eq!(ws.terminate.args["upstream"], "u:1");
	}

	#[test]
	fn reverse_proxy_websocket_paths_emits_three_rules_in_order() {
		let rules = expand(invoke(serde_json::json!({
			"upstream": "u:1",
			"websocket": ["/ws", "/api/stream"]
		})))
		.expect("expand");
		assert_eq!(
			rule_names(&rules),
			vec!["api.ws-allow", "api.ws-deny", "api.main"],
			"emission order: allow → deny → main",
		);
	}

	#[test]
	fn reverse_proxy_websocket_star_alias_treated_as_allow_all() {
		let rules =
			expand(invoke(serde_json::json!({ "upstream": "u:1", "websocket": "*" }))).expect("expand");
		assert_eq!(rule_names(&rules), vec!["api.ws", "api.main"]);
		assert_eq!(rules[0].terminate.kind, FetchKind::WebSocketUpgrade);
	}

	#[test]
	fn reverse_proxy_websocket_empty_array_rejected() {
		let err = expand(invoke(serde_json::json!({ "upstream": "u:1", "websocket": [] })))
			.expect_err("empty array invalid");
		assert!(err.to_string().contains("at least one"), "error explains: {err}");
	}

	#[test]
	fn reverse_proxy_forward_client_ip_default_true_emits_middleware() {
		let rules = expand(invoke(serde_json::json!({ "upstream": "u:1" }))).expect("expand");
		let main = find_main(&rules);
		assert!(
			main.middleware_chain.iter().any(|m| m.name == "forward_client_ip"),
			"forward_client_ip on by default",
		);
	}

	#[test]
	fn reverse_proxy_forward_client_ip_false_no_middleware() {
		let rules =
			expand(invoke(serde_json::json!({ "upstream": "u:1", "forward_client_ip": false })))
				.expect("expand");
		let main = find_main(&rules);
		assert!(main.middleware_chain.iter().all(|m| m.name != "forward_client_ip"));
	}

	#[test]
	fn reverse_proxy_rate_limit_omitted_no_middleware() {
		let rules = expand(invoke(serde_json::json!({ "upstream": "u:1" }))).expect("expand");
		let main = find_main(&rules);
		assert!(
			main.middleware_chain.iter().all(|m| m.name != "rate_limit"),
			"rate_limit not on by default — preset omits the default until the contract solidifies",
		);
	}

	#[test]
	fn reverse_proxy_rate_limit_present_emits_middleware_with_args() {
		let rules = expand(invoke(serde_json::json!({
			"upstream": "u:1",
			"rate_limit": { "rate": 50, "burst": 100 }
		})))
		.expect("expand");
		let main = find_main(&rules);
		let rl = main
			.middleware_chain
			.iter()
			.find(|m| m.name == "rate_limit")
			.expect("rate_limit ref present");
		assert_eq!(rl.args["rate"], 50);
		assert_eq!(rl.args["burst"], 100);
		assert_eq!(rl.args["window"], "1s", "default window applied");
	}

	#[test]
	fn reverse_proxy_timeouts_pass_through_to_http_proxy_args() {
		let rules = expand(invoke(serde_json::json!({
			"upstream": "u:1",
			"timeouts": { "connect": "5s", "total": "60s" }
		})))
		.expect("expand");
		let main = find_main(&rules);
		assert_eq!(main.terminate.args["timeouts"]["connect"], "5s");
		assert_eq!(main.terminate.args["timeouts"]["total"], "60s");
	}

	#[test]
	fn reverse_proxy_main_rule_is_last() {
		// Specificity ordering relies on emission stability; ws gates emit first.
		let rules = expand(invoke(serde_json::json!({
			"upstream": "u:1",
			"websocket": ["/ws"]
		})))
		.expect("expand");
		assert_eq!(rules.last().expect("non-empty").name, "api.main");
	}

	#[test]
	fn reverse_proxy_rejects_missing_upstream() {
		let err = expand(invoke(serde_json::json!({}))).expect_err("missing upstream");
		assert!(err.to_string().contains("upstream"), "error names the missing field: {err}");
	}
}
