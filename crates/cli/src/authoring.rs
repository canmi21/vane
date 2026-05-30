//! Offline config authoring — turn structured feature parameters into a
//! `rules/<name>.json` file that `vaned` will accept, without talking to
//! a running daemon.
//!
//! This is the testable core beneath both the non-interactive `vane add`
//! subcommand and the interactive `vane new` wizard: both collect the
//! same parameters and call [`author_rule`]. Validation reuses the exact
//! `vane_core` preset expansion the daemon's compile pipeline runs, so a
//! file this module writes is guaranteed to parse and expand — the CLI
//! never emits config the daemon would reject at load.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vane_core::preset::{PresetInvocation, expand_invocation};
use vane_core::rule::{SourceInfo, TlsConfig};

/// Listener-side TLS termination parameters (static cert/key on disk).
#[derive(Debug, Clone)]
pub(crate) struct TlsArgs {
	pub(crate) cert_file: String,
	pub(crate) key_file: String,
	pub(crate) sni: Option<String>,
}

/// A validated authoring request: which preset, the rule name, the
/// listen addresses, the preset-specific args blob, and optional
/// listener-side TLS termination.
#[derive(Debug, Clone)]
pub(crate) struct RuleSpec {
	pub(crate) name: String,
	pub(crate) preset: String,
	pub(crate) listen: Vec<String>,
	pub(crate) args: Value,
	pub(crate) tls: Option<TlsArgs>,
}

impl RuleSpec {
	/// Attach listener-side TLS termination to this rule (no-op when
	/// `tls` is `None`). Only meaningful for HTTP-terminating presets;
	/// the daemon's lower pass rejects TLS on an L4 listener.
	pub(crate) fn with_tls(mut self, tls: Option<TlsArgs>) -> Self {
		self.tls = tls;
		self
	}
}

/// Create (or reset) a config directory skeleton: `<dir>/rules/` and
/// `<dir>/wasm/`. Idempotent. With `force`, the `rules/` subtree is
/// cleared first so a dev loop can start from a clean slate — only the
/// vane-owned `rules/` directory is removed, never the parent or any
/// other content.
pub(crate) fn scaffold(dir: &Path, force: bool) -> anyhow::Result<()> {
	let rules = dir.join("rules");
	if force && rules.exists() {
		fs::remove_dir_all(&rules).map_err(|e| anyhow::anyhow!("clearing {}: {e}", rules.display()))?;
	}
	fs::create_dir_all(&rules).map_err(|e| anyhow::anyhow!("creating {}: {e}", rules.display()))?;
	let wasm = dir.join("wasm");
	fs::create_dir_all(&wasm).map_err(|e| anyhow::anyhow!("creating {}: {e}", wasm.display()))?;
	Ok(())
}

/// Build the [`RuleSpec`] for an L4 `port_forward` rule.
pub(crate) fn port_forward_spec(
	name: &str,
	listen: &str,
	upstream: &str,
	transport: &str,
) -> RuleSpec {
	RuleSpec {
		name: name.to_owned(),
		preset: "port_forward".to_owned(),
		listen: vec![listen.to_owned()],
		args: json!({ "upstream": upstream, "transport": transport }),
		tls: None,
	}
}

/// Build the [`RuleSpec`] for an HTTP `reverse_proxy` rule.
pub(crate) fn reverse_proxy_spec(name: &str, listen: &str, upstream: &str) -> RuleSpec {
	RuleSpec {
		name: name.to_owned(),
		preset: "reverse_proxy".to_owned(),
		listen: vec![listen.to_owned()],
		args: json!({ "upstream": upstream }),
		tls: None,
	}
}

/// Build the [`RuleSpec`] for a `static_site` fixed-response rule.
pub(crate) fn static_site_spec(name: &str, listen: &str, status: u16, body: &str) -> RuleSpec {
	RuleSpec {
		name: name.to_owned(),
		preset: "static_site".to_owned(),
		listen: vec![listen.to_owned()],
		args: json!({ "status": status, "body": body }),
		tls: None,
	}
}

/// Validate `spec` against the real preset expander, then write it to
/// `<config_dir>/rules/<name>.json` as the canonical
/// `{ "rules": [ <invocation> ] }` shape. Returns the path written.
///
/// # Errors
/// Returns an error if preset expansion rejects the parameters (e.g. an
/// unknown preset or an invalid arg like `transport: "sctp"`), or if the
/// file cannot be written.
pub(crate) fn author_rule(config_dir: &Path, spec: &RuleSpec) -> anyhow::Result<PathBuf> {
	// Validate offline by running the same expansion the daemon's compile
	// pipeline uses. This catches bad args without a daemon or the engine.
	let inv = PresetInvocation {
		name: spec.name.clone(),
		preset: spec.preset.clone(),
		listen: spec.listen.clone(),
		args: spec.args.clone(),
		tls: spec.tls.as_ref().map(to_tls_config),
		source: SourceInfo::default(),
	};
	expand_invocation(inv).map_err(|e| anyhow::anyhow!("invalid rule: {e}"))?;

	let rules_dir = config_dir.join("rules");
	fs::create_dir_all(&rules_dir)
		.map_err(|e| anyhow::anyhow!("creating {}: {e}", rules_dir.display()))?;
	let path = rules_dir.join(format!("{}.json", spec.name));

	// Serialize the clean operator-facing shape — no internal `source`
	// noise — matching what a hand-authored rule file looks like.
	let mut entry = json!({
		"preset": spec.preset,
		"name": spec.name,
		"listen": spec.listen,
		"args": spec.args,
	});
	if let Some(t) = &spec.tls {
		entry["tls"] = tls_json(t);
	}
	let doc = json!({ "rules": [entry] });
	let body = serde_json::to_string_pretty(&doc)?;
	fs::write(&path, format!("{body}\n"))
		.map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
	Ok(path)
}

/// Build a static-cert [`TlsConfig`] for offline validation. `enable_zero_rtt`
/// is `false` (no early data); mTLS / OCSP are off — the simple HTTPS case.
fn to_tls_config(t: &TlsArgs) -> TlsConfig {
	TlsConfig {
		sni: t.sni.clone(),
		cert_file: Some(PathBuf::from(&t.cert_file)),
		key_file: Some(PathBuf::from(&t.key_file)),
		managed: None,
		enable_zero_rtt: false,
		client_auth: None,
		ocsp_path: None,
		ocsp_fetch: false,
	}
}

/// The operator-facing `tls` JSON block. `enable_zero_rtt` has no serde
/// default, so it must be emitted explicitly on every TLS-terminating rule.
fn tls_json(t: &TlsArgs) -> Value {
	let mut v = json!({
		"cert_file": t.cert_file,
		"key_file": t.key_file,
		"enable_zero_rtt": false,
	});
	if let Some(sni) = &t.sni {
		v["sni"] = json!(sni);
	}
	v
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn port_forward_authors_a_loadable_rule_file() {
		let tmp = tempfile::tempdir().expect("tempdir");
		scaffold(tmp.path(), false).expect("scaffold");
		let spec = port_forward_spec("ssh-fwd", "127.0.0.1:2222", "127.0.0.1:22", "tcp");
		let path = author_rule(tmp.path(), &spec).expect("author");

		assert_eq!(path, tmp.path().join("rules/ssh-fwd.json"));
		let written: Value =
			serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse json");
		assert_eq!(written["rules"][0]["preset"], "port_forward");
		assert_eq!(written["rules"][0]["listen"][0], "127.0.0.1:2222");
		assert_eq!(written["rules"][0]["args"]["upstream"], "127.0.0.1:22");
	}

	#[test]
	fn invalid_transport_is_rejected_before_write() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let spec = port_forward_spec("bad", ":2222", "1.2.3.4:53", "sctp");
		let err = author_rule(tmp.path(), &spec).expect_err("sctp must reject");
		assert!(err.to_string().contains("sctp"), "error names the bad value: {err}");
		assert!(!tmp.path().join("rules/bad.json").exists(), "no file on validation failure");
	}

	#[test]
	fn force_scaffold_clears_prior_rules() {
		let tmp = tempfile::tempdir().expect("tempdir");
		scaffold(tmp.path(), false).expect("scaffold");
		let spec = port_forward_spec("old", ":2222", "1.2.3.4:22", "tcp");
		author_rule(tmp.path(), &spec).expect("author");
		assert!(tmp.path().join("rules/old.json").exists());

		scaffold(tmp.path(), true).expect("reset");
		assert!(!tmp.path().join("rules/old.json").exists(), "force clears rules/");
		assert!(tmp.path().join("rules").is_dir(), "rules/ recreated");
	}
}
