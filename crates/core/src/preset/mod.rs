//! Preset expansion: `{"preset": ..., ...}` → `Vec<RawRule>`.
//!
//! Presets are opinionated compile-stage expansions that turn high-level
//! intent into raw-rule bundles. The four MVP presets are
//! `reverse_proxy`, `port_forward`, `static_site`, and `redirect_https`.
//!
//! See `spec/architecture/14-presets.md`. Feature: S1-22.

mod port_forward;
mod redirect_https;
mod reverse_proxy;
mod static_site;

use serde_json::Value;

use crate::error::Error;
use crate::rule::{ListenSpec, RawRule, SourceInfo};

/// User-authored preset invocation. The `preset` field discriminates
/// which expander runs; `args` is opaque at parse time and validated
/// inside the expander.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PresetInvocation {
	/// Base name; the expander prefixes synth rules (`<name>.main`,
	/// `<name>.ws`, `<name>.ws-allow`, `<name>.ws-deny`).
	pub name: String,
	/// Discriminator. One of `reverse_proxy` / `port_forward` /
	/// `static_site` / `redirect_https`.
	pub preset: String,
	pub listen: Vec<ListenSpec>,
	#[serde(default)]
	pub args: Value,
	#[serde(default)]
	pub source: SourceInfo,
}

/// File-level entry: either a hand-written raw rule or a preset
/// invocation that expands to one or more raw rules. Discrimination is
/// by presence of the top-level `preset` key — the custom `Deserialize`
/// peeks at the JSON before routing to the right variant so a malformed
/// preset payload produces a pointed error instead of falling through to
/// `RawRule` parsing and surfacing a confusing "missing terminate" error.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum RuleEntry {
	Preset(PresetInvocation),
	Raw(RawRule),
}

impl<'de> serde::Deserialize<'de> for RuleEntry {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		let v = Value::deserialize(d)?;
		if v.get("preset").is_some() {
			let inv: PresetInvocation = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
			Ok(Self::Preset(inv))
		} else {
			let r: RawRule = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
			Ok(Self::Raw(r))
		}
	}
}

/// Dispatch on `inv.preset` to the appropriate expander.
///
/// # Errors
/// Returns [`Error::compile`] when `inv.preset` names an unknown preset,
/// or when the dispatched expander rejects `inv.args`.
pub fn expand_invocation(inv: PresetInvocation) -> Result<Vec<RawRule>, Error> {
	match inv.preset.as_str() {
		"reverse_proxy" => reverse_proxy::expand(inv),
		"port_forward" => port_forward::expand(inv),
		"static_site" => static_site::expand(inv),
		"redirect_https" => redirect_https::expand(inv),
		other => Err(Error::compile(format!(
			"unknown preset {other:?}; supported: reverse_proxy / port_forward / static_site / redirect_https"
		))),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn unknown_preset_name_yields_compile_error() {
		let inv = PresetInvocation {
			name: "x".into(),
			preset: "no_such_preset".into(),
			listen: vec![":443".into()],
			args: Value::Null,
			source: SourceInfo::default(),
		};
		let err = expand_invocation(inv).expect_err("unknown preset must fail");
		let msg = err.to_string();
		assert!(msg.contains("no_such_preset"), "error names the offending preset: {msg}");
		assert!(msg.contains("reverse_proxy"), "error lists supported presets: {msg}");
	}

	#[test]
	fn rule_entry_deserializes_preset_when_preset_key_present() {
		let raw = serde_json::json!({
			"preset": "port_forward",
			"name": "ssh",
			"listen": [":2222"],
			"args": { "upstream": "10.0.0.5:22" }
		});
		let entry: RuleEntry = serde_json::from_value(raw).expect("parse preset entry");
		match entry {
			RuleEntry::Preset(inv) => {
				assert_eq!(inv.preset, "port_forward");
				assert_eq!(inv.name, "ssh");
				assert_eq!(inv.listen, vec![":2222".to_string()]);
			}
			RuleEntry::Raw(_) => panic!("preset key must route to Preset variant"),
		}
	}

	#[test]
	fn rule_entry_deserializes_raw_when_no_preset_key() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" }
		});
		let entry: RuleEntry = serde_json::from_value(raw).expect("parse raw entry");
		match entry {
			RuleEntry::Raw(r) => assert_eq!(r.name, "r"),
			RuleEntry::Preset(_) => panic!("no preset key must route to Raw variant"),
		}
	}
}
