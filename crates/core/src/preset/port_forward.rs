//! `port_forward` preset — raw L4 byte forward (TCP or UDP).
//!
//! Expands to a single `RawRule` whose terminate is `L4Forward`. No
//! middleware — there is no HTTP layer to set headers on. See
//! `spec/crates/core.md` § _Compile pipeline_.

use crate::error::Error;
use crate::fetch::FetchKind;
use crate::preset::PresetInvocation;
use crate::rule::{RawRule, TerminateSpec};

#[derive(serde::Deserialize)]
struct Args {
	upstream: String,
	#[serde(default = "default_transport")]
	transport: String,
}

fn default_transport() -> String {
	"tcp".to_string()
}

pub(super) fn expand(inv: PresetInvocation) -> Result<Vec<RawRule>, Error> {
	let args: Args = serde_json::from_value(inv.args.clone())
		.map_err(|e| Error::compile(format!("preset port_forward args: {e}")))?;
	if !matches!(args.transport.as_str(), "tcp" | "udp") {
		return Err(Error::compile(format!(
			"preset port_forward: transport must be \"tcp\" or \"udp\", got {:?}",
			args.transport
		)));
	}
	let terminate_args = serde_json::json!({
		"upstream": args.upstream,
		"transport": args.transport,
	});
	// Presets emit `allow_zero_rtt` explicitly per `spec/crates/engine-tls.md` § _TLS
	// 1.3 0-RTT_'s "CLI / TUI emits `false` when 0-RTT is not in use".
	// `port_forward` is L4 only; the lower pass rejects an L4 rule with
	// `allow_zero_rtt` set, so emit `None` regardless of `inv.tls`.
	let _ = inv.tls.as_ref();
	Ok(vec![RawRule {
		name: inv.name,
		listen: inv.listen,
		match_predicate: None,
		middleware_chain: vec![],
		terminate: TerminateSpec { kind: FetchKind::L4Forward, args: terminate_args },
		// `lower_port` rejects L4 listeners with `tls` set — TLS
		// termination on a byte-tunnel makes no sense (vane decrypts
		// then forwards plaintext to upstream, leaking the channel).
		// Propagating the user's value here keeps the error message
		// pointed at the rule rather than silently dropping it.
		tls: inv.tls,
		allow_zero_rtt: None,
		max_body_bytes_request: 8 * 1024 * 1024,
		max_body_bytes_response: 8 * 1024 * 1024,
		source: inv.source,
	}])
}

#[cfg(test)]
mod tests {
	use std::path::PathBuf;

	use serde_json::Value;

	use super::*;
	use crate::rule::SourceInfo;

	fn invoke(args: Value) -> PresetInvocation {
		PresetInvocation {
			name: "fwd".to_string(),
			preset: "port_forward".to_string(),
			listen: vec![":2222".into()],
			args,
			tls: None,
			source: SourceInfo { file: PathBuf::from("rules/x.json"), line: 3 },
		}
	}

	#[test]
	fn port_forward_tcp_expands_to_single_l4forward_rule() {
		let rules = expand(invoke(serde_json::json!({
			"upstream": "10.0.0.5:22",
			"transport": "tcp"
		})))
		.expect("expand");
		assert_eq!(rules.len(), 1, "port_forward emits exactly one rule");
		let r = &rules[0];
		assert_eq!(r.name, "fwd");
		assert_eq!(r.terminate.kind, FetchKind::L4Forward);
		assert_eq!(
			r.terminate.args,
			serde_json::json!({ "upstream": "10.0.0.5:22", "transport": "tcp" })
		);
		assert!(r.middleware_chain.is_empty(), "L4 forwarding has no middleware");
		assert!(r.match_predicate.is_none(), "port_forward never emits a match predicate");
	}

	#[test]
	fn port_forward_udp_alias_expands_correctly() {
		let rules = expand(invoke(serde_json::json!({ "upstream": "1.2.3.4:53", "transport": "udp" })))
			.expect("expand");
		assert_eq!(rules[0].terminate.args["transport"], "udp");
	}

	#[test]
	fn port_forward_default_transport_is_tcp() {
		let rules = expand(invoke(serde_json::json!({ "upstream": "10.0.0.5:22" }))).expect("expand");
		assert_eq!(rules[0].terminate.args["transport"], "tcp");
	}

	#[test]
	fn port_forward_rejects_invalid_transport_string() {
		let err =
			expand(invoke(serde_json::json!({ "upstream": "x", "transport": "sctp" }))).expect_err("");
		assert!(err.to_string().contains("sctp"), "error names offending value: {err}");
	}

	#[test]
	fn port_forward_preserves_listen_and_source() {
		let rules = expand(invoke(serde_json::json!({ "upstream": "10.0.0.5:22" }))).expect("expand");
		assert_eq!(rules[0].listen, vec![":2222".to_string()]);
		assert_eq!(rules[0].source.file, PathBuf::from("rules/x.json"));
		assert_eq!(rules[0].source.line, 3);
	}
}
