//! `vane new` — interactive, clack-style config wizard.
//!
//! A thin interactive shell over the [`crate::authoring`] core: it
//! collects the same parameters the non-interactive `vane add` takes,
//! one question at a time, then writes a validated rule file. Requires a
//! TTY; scripts and automation should drive `vane add` instead.

use std::path::PathBuf;

use crate::authoring;

/// Expand a leading `~/` to `$HOME` so a typed `~/vane-dev` works (the
/// wizard reads raw text, so the shell never gets a chance to expand it).
fn expand_tilde(s: &str) -> PathBuf {
	if let Some(rest) = s.strip_prefix("~/")
		&& let Ok(home) = std::env::var("HOME")
	{
		return PathBuf::from(home).join(rest);
	}
	PathBuf::from(s)
}

/// Run the wizard: pick a feature, answer its questions, write the rule.
pub(crate) fn run() -> anyhow::Result<()> {
	cliclack::intro("vane new")?;

	let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
	let default_dir = format!("{home}/vane-dev");
	let dir_in: String =
		cliclack::input("Config directory").default_input(&default_dir).interact()?;
	let dir = expand_tilde(&dir_in);

	let feature: &str = cliclack::select("Feature")
		.item("port_forward", "Port forward (L4)", "raw TCP/UDP byte forward")
		.item("reverse_proxy", "Reverse proxy (HTTP)", "forward HTTP to an upstream")
		.item("static_site", "Static response", "return a fixed response")
		.interact()?;

	let name: String =
		cliclack::input("Rule name").default_input(default_name(feature)).interact()?;

	let listen: String =
		cliclack::input("Listen address").default_input(default_listen(feature)).interact()?;

	let spec = match feature {
		"port_forward" => {
			let upstream: String =
				cliclack::input("Forward to (upstream)").default_input("127.0.0.1:9000").interact()?;
			let transport: &str =
				cliclack::select("Transport").item("tcp", "tcp", "").item("udp", "udp", "").interact()?;
			authoring::port_forward_spec(&name, &listen, &upstream, transport)
		}
		"reverse_proxy" => {
			let upstream: String =
				cliclack::input("Upstream").default_input("127.0.0.1:9000").interact()?;
			authoring::reverse_proxy_spec(&name, &listen, &upstream).with_tls(prompt_tls()?)
		}
		"static_site" => {
			let status: u16 = cliclack::input("Status code").default_input("200").interact()?;
			let body: String = cliclack::input("Body").default_input("hello from vane").interact()?;
			authoring::static_site_spec(&name, &listen, status, &body).with_tls(prompt_tls()?)
		}
		other => anyhow::bail!("unhandled feature {other:?}"),
	};

	authoring::scaffold(&dir, false)?;
	let path = authoring::author_rule(&dir, &spec)?;

	cliclack::outro(format!("wrote {}\n   start: vaned -c {}", path.display(), dir.display()))?;
	Ok(())
}

/// Optional TLS-termination step for HTTP-terminating features: ask
/// whether to serve HTTPS, and if so collect the cert/key PEM paths and
/// an optional SNI host.
fn prompt_tls() -> anyhow::Result<Option<authoring::TlsArgs>> {
	let enable: bool =
		cliclack::confirm("Serve over HTTPS (TLS termination)?").initial_value(false).interact()?;
	if !enable {
		return Ok(None);
	}
	let cert_file: String =
		cliclack::input("TLS cert chain PEM path").placeholder("/path/to/fullchain.pem").interact()?;
	let key_file: String =
		cliclack::input("TLS private key PEM path").placeholder("/path/to/key.pem").interact()?;
	let sni_in: String =
		cliclack::input("SNI host (blank = listener default cert)").default_input("").interact()?;
	let sni = if sni_in.trim().is_empty() { None } else { Some(sni_in) };
	Ok(Some(authoring::TlsArgs { cert_file, key_file, sni }))
}

fn default_name(feature: &str) -> &'static str {
	match feature {
		"port_forward" => "fwd",
		"reverse_proxy" => "proxy",
		"static_site" => "site",
		_ => "rule",
	}
}

fn default_listen(feature: &str) -> &'static str {
	match feature {
		"port_forward" => "127.0.0.1:2222",
		_ => "127.0.0.1:8080",
	}
}
