use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::error::Error;
use crate::fetch::FetchKind;
use crate::predicate::Predicate;

pub type ListenSpec = String;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RawRule {
	pub name: String,
	#[serde(deserialize_with = "de_listen_non_empty")]
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
	/// `spec/crates/engine-tls.md` § _TLS 1.3 0-RTT (early data)_.
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

/// Reject `listen: []` at parse time. An empty listen list silently
/// drops the rule from every listener pool, which is almost always an
/// operator mistake — surface it before the rule reaches lower / link.
pub(crate) fn de_listen_non_empty<'de, D: serde::Deserializer<'de>>(
	d: D,
) -> Result<Vec<ListenSpec>, D::Error> {
	let v: Vec<ListenSpec> = serde::Deserialize::deserialize(d)?;
	if v.is_empty() {
		return Err(serde::de::Error::custom(
			"rule `listen` must not be empty; specify at least one address",
		));
	}
	Ok(v)
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
/// boundary per spec/crates/engine-tls.md § _SNI peek (L4, no decrypt)_; comparison against
/// rustls's already-lowercased `ClientHello::server_name()` is then
/// byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct TlsConfig {
	#[serde(default)]
	pub sni: Option<String>,
	/// Path to the leaf+chain PEM. Required when the cert is operator-
	/// supplied (static); absent when the cert comes from `managed`.
	/// Per-rule validation enforces "exactly one of static paths or
	/// `managed`"; lower-pass branches on the result.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cert_file: Option<PathBuf>,
	/// Path to the private key PEM. Same lifecycle as `cert_file`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub key_file: Option<PathBuf>,
	/// ACME-managed cert source. When set, `cert_file` / `key_file`
	/// must be absent. The compiler routes this rule into the
	/// listener's `managed_snis` table; the engine's
	/// `ManagedCertPopulator` supplies the actual cert.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub managed: Option<ManagedSpec>,
	/// Listener-side TLS 1.3 0-RTT opt-in. Required on every rule that
	/// carries a `tls` block; rules sharing one listener must agree on
	/// this value (lower aggregates them). See
	/// `spec/crates/engine-tls.md` § _TLS 1.3 0-RTT (early data)_.
	pub enable_zero_rtt: bool,
	/// Listener-side mTLS — per `spec/crates/engine-tls.md` § _Client certificate verification (mTLS on listener)_. Per-rule input; the lower pass aggregates each
	/// rule's `client_auth` into one `ClientAuthSpec` per listener
	/// address (rules on the same listener must agree, else compile
	/// error). `None` keeps the listener at `ClientAuth::None`.
	#[serde(default)]
	pub client_auth: Option<ClientAuthConfig>,
	/// Path to a pre-fetched OCSP response (DER) on disk. The
	/// populator reads this file at every refresh and stages the
	/// bytes into the resolver. Useful for HTTPS-only OCSP
	/// responders (which `vane` does not fetch from — see
	/// `spec/crates/engine-tls.md` § _OCSP stapling_) and for
	/// air-gapped deployments where the operator cron-runs
	/// `openssl ocsp` themselves. Mutually exclusive with
	/// [`Self::ocsp_fetch`].
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub ocsp_path: Option<PathBuf>,
	/// When `true`, the populator extracts the OCSP responder URL
	/// from the cert's AIA extension and fetches the response over
	/// HTTP at refresh time. HTTP-only by policy (per
	/// `spec/crates/engine-tls.md` § _OCSP stapling_).
	/// Mutually exclusive with [`Self::ocsp_path`].
	#[serde(default, skip_serializing_if = "is_default_false")]
	pub ocsp_fetch: bool,
}

#[allow(
	clippy::trivially_copy_pass_by_ref,
	reason = "serde skip_serializing_if requires fn(&T) -> bool"
)]
fn is_default_false(b: &bool) -> bool {
	!*b
}

impl TlsConfig {
	/// `true` when this `tls` block routes through ACME, not static disk
	/// paths. Inverse of [`Self::is_static`].
	#[must_use]
	pub const fn is_managed(&self) -> bool {
		self.managed.is_some()
	}

	/// `true` when both `cert_file` and `key_file` are present and
	/// `managed` is absent. The lower pass guarantees this for every
	/// `TlsConfig` it stores in [`ListenerTlsSpec::default`] /
	/// [`ListenerTlsSpec::sni_certs`], so static-cert consumers can
	/// rely on the static-paths invariant downstream.
	#[must_use]
	pub const fn is_static(&self) -> bool {
		self.managed.is_none() && self.cert_file.is_some() && self.key_file.is_some()
	}

	/// Static cert paths if this is a static config. The lower pass
	/// guarantees `(cert_file, key_file)` are both `Some` whenever
	/// `managed` is `None`, so this returns `Some` for every
	/// post-lower static `TlsConfig`.
	#[must_use]
	pub fn static_paths(&self) -> Option<(&Path, &Path)> {
		match (&self.cert_file, &self.key_file, &self.managed) {
			(Some(c), Some(k), None) => Some((c.as_path(), k.as_path())),
			_ => None,
		}
	}

	/// Per-rule pre-lower validation per `spec/crates/engine-acme.md` § _Configuration schema_ and `spec/crates/engine-tls.md` § _Upstream-side TLS_:
	///
	/// 1. Exactly one of (`cert_file` ∧ `key_file`) or `managed` is
	///    present.
	/// 2. When `managed` is set, every required `ManagedSpec` invariant
	///    holds: `agree_tos == true`, non-empty `contact`, non-empty
	///    `san`, `tls.sni ∈ san`, no wildcard SAN unless `dns-01`,
	///    `dns-01` ⇒ `dns_provider`, `renew_before` parses to a
	///    positive `Duration`.
	///
	/// # Errors
	/// Returns [`Error::compile`] with a single sentence pointing at
	/// the offending field. The error string is operator-readable —
	/// the `vane compile` UI surfaces it verbatim.
	pub fn validate(&self) -> Result<(), Error> {
		// OCSP source mutex per `spec/crates/engine-tls.md` § _OCSP stapling_:
		// `ocsp_path` and `ocsp_fetch` are independent strategies
		// for the same goal and must not both be set on one rule.
		// We check this before the cert-source branching so the
		// error message points operators at OCSP rather than the
		// cert-mode confusion that would otherwise mask it.
		if self.ocsp_path.is_some() && self.ocsp_fetch {
			return Err(Error::compile(
				"tls: `ocsp_path` and `ocsp_fetch` are mutually exclusive — pick one OCSP source",
			));
		}
		let static_present = self.cert_file.is_some() || self.key_file.is_some();
		match (static_present, &self.managed) {
			(true, Some(_)) => Err(Error::compile(
				"tls: `managed` must not coexist with `cert_file` / `key_file` — pick one source",
			)),
			(false, None) => Err(Error::compile(
				"tls: missing cert source — set either `cert_file` + `key_file` or `managed`",
			)),
			(true, None) => match (&self.cert_file, &self.key_file) {
				(Some(_), Some(_)) => Ok(()),
				(Some(_), None) => {
					Err(Error::compile("tls: `key_file` is required when `cert_file` is set"))
				}
				(None, Some(_)) => {
					Err(Error::compile("tls: `cert_file` is required when `key_file` is set"))
				}
				(None, None) => unreachable!("static_present implies one path is Some"),
			},
			(false, Some(m)) => m.validate(self.sni.as_deref()),
		}
	}
}

/// ACME-managed cert spec — operator-supplied, parsed verbatim from
/// `tls.managed` per `spec/crates/engine-acme.md` § _Configuration schema_.
///
/// Every required field is mandatory in JSON: there are no implicit
/// defaults, since the JSON is generated by `vane`'s CLI / TUI rather
/// than hand-written. Defaulting in the schema would let a regression
/// silently swap directory URLs (LE prod vs staging) or key types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ManagedSpec {
	pub directory_url: String,
	pub contact: Vec<String>,
	pub agree_tos: bool,
	pub challenge: ChallengeKind,
	pub key_type: ManagedKeyType,
	/// Renewal anticipation: kick off renewal when
	/// `now + renew_before >= not_after`. Duration grammar mirrors
	/// `rate_limit.window` (extended with `h` and `d` units —
	/// renewal windows are typically days, not minutes).
	pub renew_before: String,
	pub san: Vec<String>,
	/// BYO account key (PEM PKCS#8). When absent, the registry
	/// auto-creates and persists via `AcmeStore::save_account`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub account_key_path: Option<PathBuf>,
	/// DNS provider config — required when `challenge == "dns-01"`,
	/// must be absent for `http-01`. The schema is provider-specific
	/// (Cargo-feature-gated parser); core stores the raw JSON.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub dns_provider: Option<Value>,
}

impl ManagedSpec {
	/// Parsed `renew_before`. Re-parses on every call; callers that
	/// need the value hot-path-frequent should cache it.
	///
	/// # Errors
	/// Returns [`Error::compile`] when the literal is malformed or
	/// non-positive.
	pub fn renew_before_duration(&self) -> Result<Duration, Error> {
		parse_renewal_duration(&self.renew_before)
	}

	/// Per-rule invariants, called from [`TlsConfig::validate`].
	///
	/// `tls_sni` is the parent rule's `tls.sni`; `spec/crates/engine-acme.md` § _Configuration schema_ requires `san ⊇ {tls.sni}`.
	///
	/// # Errors
	/// One [`Error::compile`] per violation, in declaration order.
	fn validate(&self, tls_sni: Option<&str>) -> Result<(), Error> {
		if !self.agree_tos {
			return Err(Error::compile("tls.managed.agree_tos must be true"));
		}
		if self.contact.is_empty() {
			return Err(Error::compile("tls.managed.contact must list at least one URI"));
		}
		if self.directory_url.trim().is_empty() {
			return Err(Error::compile("tls.managed.directory_url must not be empty"));
		}
		if self.san.is_empty() {
			return Err(Error::compile("tls.managed.san must list at least one name"));
		}
		match tls_sni {
			Some(sni) if !self.san.iter().any(|s| s.eq_ignore_ascii_case(sni)) => {
				return Err(Error::compile(format!("tls.managed.san must contain tls.sni ({sni:?})")));
			}
			None => {
				return Err(Error::compile("tls.managed requires tls.sni — managed certs are SNI-keyed"));
			}
			Some(_) => {}
		}
		match (self.challenge, self.dns_provider.is_some()) {
			(ChallengeKind::Dns01, false) => {
				return Err(Error::compile("tls.managed: challenge \"dns-01\" requires `dns_provider`"));
			}
			(ChallengeKind::Http01, true) => {
				return Err(Error::compile(
					"tls.managed: `dns_provider` is only meaningful when challenge == \"dns-01\"",
				));
			}
			_ => {}
		}
		if matches!(self.challenge, ChallengeKind::Http01) {
			for san in &self.san {
				if san.starts_with("*.") {
					return Err(Error::compile(format!(
						"tls.managed: wildcard SAN {san:?} requires challenge \"dns-01\""
					)));
				}
			}
		}
		let renew = self.renew_before_duration()?;
		if renew.is_zero() {
			return Err(Error::compile("tls.managed.renew_before must be > 0"));
		}
		Ok(())
	}
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub enum ChallengeKind {
	#[serde(rename = "http-01")]
	Http01,
	#[serde(rename = "dns-01")]
	Dns01,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub enum ManagedKeyType {
	#[serde(rename = "ecdsa-p256")]
	EcdsaP256,
	#[serde(rename = "rsa-2048")]
	Rsa2048,
}

/// Parse a duration literal of the form `<integer><unit>` where
/// `unit ∈ { "ms", "s", "m", "h", "d" }`. Mirrors the
/// `rate_limit.window` grammar (`engine/src/fetch/retry.rs`),
/// extended with `h` and `d` because renewal windows are typically
/// expressed in days. Hand-rolled to avoid pulling `humantime` into
/// `vane-core`.
///
/// # Errors
/// Returns [`Error::compile`] when the literal is empty, missing a
/// unit, or has a non-integer numeric portion.
fn parse_renewal_duration(s: &str) -> Result<Duration, Error> {
	let s = s.trim();
	if s.is_empty() {
		return Err(Error::compile("duration must be non-empty"));
	}
	let (num, unit_secs) = if let Some(rest) = s.strip_suffix("ms") {
		(rest, None) // milliseconds — special-cased below
	} else if let Some(rest) = s.strip_suffix('s') {
		(rest, Some(1u64))
	} else if let Some(rest) = s.strip_suffix('m') {
		(rest, Some(60u64))
	} else if let Some(rest) = s.strip_suffix('h') {
		(rest, Some(60 * 60))
	} else if let Some(rest) = s.strip_suffix('d') {
		(rest, Some(60 * 60 * 24))
	} else {
		return Err(Error::compile(format!(
			"duration {s:?}: missing unit (expected ms / s / m / h / d)"
		)));
	};
	let n: u64 = num.trim().parse().map_err(|e| Error::compile(format!("duration {s:?}: {e}")))?;
	Ok(match unit_secs {
		None => Duration::from_millis(n),
		Some(secs) => Duration::from_secs(n.saturating_mul(secs)),
	})
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
/// `fetch_failure` policy. Bytes are owned by the daemon-wide CRL
/// cache (`vane_engine::tls::CrlCache`); this struct only carries
/// the parsed schema.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CrlSourceConfig {
	File { path: PathBuf, fetch_failure: CrlFetchFailure },
	Url { url: String, fetch_failure: CrlFetchFailure },
}

/// CRL availability policy (per `spec/crates/engine-tls.md` § _CRL_).
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
	/// ACME-managed certs declared on this listener, keyed by SNI
	/// (lowercased). The lower pass populates this from rules whose
	/// `tls.managed` is set; the daemon's `ManagedCertRegistry`
	/// picks them up and delivers actual certs through the listener's
	/// `ManagedCertPopulator`.
	///
	/// This map is the source of truth for boot-time issuance — every
	/// entry triggers a one-shot `issue_http01` attempt — and feeds
	/// the renewal scheduler.
	#[serde(default)]
	pub managed_snis: BTreeMap<String, ManagedSpec>,
	/// Resolved per-listener mTLS policy. Per `spec/crates/engine-tls.md` § _Client certificate verification (mTLS on listener)_ this is per-listener, derived from the
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
	/// `spec/crates/engine-tls.md` § _TLS 1.3 0-RTT (early data)_.
	#[serde(default)]
	pub enable_zero_rtt: bool,
}

impl ListenerTlsSpec {
	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.default.is_none()
			&& self.sni_certs.is_empty()
			&& self.managed_snis.is_empty()
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
		// spec/crates/engine.md `spec/crates/engine.md` § _Concrete fetches_:
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
	fn raw_rule_with_empty_listen_is_rejected_at_deserialize() {
		let raw = serde_json::json!({
			"name": "r",
			"listen": [],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" },
		});
		let err = serde_json::from_value::<RawRule>(raw).expect_err("empty listen must reject");
		let msg = err.to_string();
		assert!(msg.contains("listen") && msg.contains("not be empty"), "{msg}");
	}

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
		// Every row of spec/crates/engine.md `spec/crates/engine.md` § _Concrete fetches_.
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
		// spec/crates/core.md § _Compile pipeline_: "every other key goes into `args`
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
		// spec/crates/core.md § _Compile pipeline_: the custom Deserialize removes `type`
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
		assert_eq!(tls.cert_file.as_deref(), Some(Path::new("/etc/vaned/certs/api.pem")));
		assert_eq!(tls.key_file.as_deref(), Some(Path::new("/etc/vaned/certs/api.key")));
		assert!(!tls.enable_zero_rtt);
	}

	#[test]
	fn tls_config_round_trips_through_json() {
		let original = TlsConfig {
			sni: None,
			cert_file: Some(PathBuf::from("/srv/cert.pem")),
			key_file: Some(PathBuf::from("/srv/key.pem")),
			managed: None,
			enable_zero_rtt: false,
			client_auth: None,
			ocsp_path: None,
			ocsp_fetch: false,
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
		// `spec/crates/engine-tls.md` § _TLS 1.3 0-RTT (early data)_; absence on a `tls` block is a
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

	// `tls.managed` schema + validation. Each test exercises one of the
	// compile-time invariants from `spec/crates/engine-acme.md`
	// § _Configuration schema_, plus parser round-trips.
	// `TlsConfig::validate` returns the first violation in declaration
	// order; tests assert on the substring rather than the full message
	// so wording can evolve without churning fixtures.

	fn managed_tls(challenge: &str, with_dns_provider: bool) -> serde_json::Value {
		let mut managed = serde_json::json!({
			"directory_url": "https://acme-staging-v02.api.letsencrypt.org/directory",
			"contact": ["mailto:ops@example.com"],
			"agree_tos": true,
			"challenge": challenge,
			"key_type": "ecdsa-p256",
			"renew_before": "30d",
			"san": ["api.example.com"],
		});
		if with_dns_provider {
			managed["dns_provider"] = serde_json::json!({ "kind": "cloudflare" });
		}
		serde_json::json!({
			"sni": "api.example.com",
			"managed": managed,
			"enable_zero_rtt": false,
		})
	}

	#[test]
	fn tls_managed_round_trips_through_json() {
		let raw = managed_tls("http-01", false);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse managed");
		let m = tls.managed.as_ref().expect("managed");
		assert!(m.agree_tos);
		assert_eq!(m.challenge, ChallengeKind::Http01);
		assert_eq!(m.key_type, ManagedKeyType::EcdsaP256);
		assert_eq!(m.san, vec!["api.example.com".to_owned()]);
		assert_eq!(m.contact, vec!["mailto:ops@example.com".to_owned()]);
		assert!(m.dns_provider.is_none());
		assert!(tls.is_managed());
		assert!(!tls.is_static());
	}

	#[test]
	fn tls_managed_validates_happy_path() {
		let raw = managed_tls("http-01", false);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		tls.validate().expect("happy path validates");
	}

	#[test]
	fn tls_validate_rejects_both_static_and_managed() {
		let raw = serde_json::json!({
			"sni": "api.example.com",
			"cert_file": "/tmp/cert.pem",
			"key_file": "/tmp/key.pem",
			"managed": {
				"directory_url": "https://example",
				"contact": ["mailto:ops@example.com"],
				"agree_tos": true,
				"challenge": "http-01",
				"key_type": "ecdsa-p256",
				"renew_before": "30d",
				"san": ["api.example.com"],
			},
			"enable_zero_rtt": false,
		});
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("must not coexist"), "{err}");
	}

	#[test]
	fn tls_validate_rejects_neither_static_nor_managed() {
		let raw = serde_json::json!({ "enable_zero_rtt": false });
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("missing cert source"), "{err}");
	}

	#[test]
	fn tls_validate_rejects_partial_static_paths() {
		let raw = serde_json::json!({
			"cert_file": "/tmp/cert.pem",
			"enable_zero_rtt": false,
		});
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("`key_file`"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_agree_tos_false() {
		let mut raw = managed_tls("http-01", false);
		raw["managed"]["agree_tos"] = serde_json::Value::Bool(false);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("agree_tos must be true"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_dns01_without_dns_provider() {
		let raw = managed_tls("dns-01", false);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("requires `dns_provider`"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_http01_with_dns_provider() {
		let raw = managed_tls("http-01", true);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("dns_provider"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_wildcard_san_with_http01() {
		let mut raw = managed_tls("http-01", false);
		raw["managed"]["san"] = serde_json::json!(["*.example.com", "api.example.com"]);
		raw["sni"] = serde_json::Value::String("api.example.com".to_owned());
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("wildcard"), "{err}");
	}

	#[test]
	fn tls_managed_accepts_wildcard_san_with_dns01() {
		let mut raw = managed_tls("dns-01", true);
		raw["managed"]["san"] = serde_json::json!(["*.example.com", "api.example.com"]);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		tls.validate().expect("dns-01 wildcard ok");
	}

	#[test]
	fn tls_managed_rejects_san_missing_sni() {
		let mut raw = managed_tls("http-01", false);
		raw["sni"] = serde_json::Value::String("other.example.com".to_owned());
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("must contain tls.sni"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_missing_sni() {
		let mut raw = managed_tls("http-01", false);
		raw.as_object_mut().expect("obj").remove("sni");
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("requires tls.sni"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_empty_contact() {
		let mut raw = managed_tls("http-01", false);
		raw["managed"]["contact"] = serde_json::json!([]);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("contact must list"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_empty_san() {
		let mut raw = managed_tls("http-01", false);
		raw["managed"]["san"] = serde_json::json!([]);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("san must list"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_empty_directory_url() {
		let mut raw = managed_tls("http-01", false);
		raw["managed"]["directory_url"] = serde_json::Value::String(String::new());
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("directory_url"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_zero_renew_before() {
		let mut raw = managed_tls("http-01", false);
		raw["managed"]["renew_before"] = serde_json::Value::String("0d".to_owned());
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("must be > 0"), "{err}");
	}

	#[test]
	fn tls_managed_rejects_unparseable_renew_before() {
		let mut raw = managed_tls("http-01", false);
		raw["managed"]["renew_before"] = serde_json::Value::String("garbage".to_owned());
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let err = tls.validate().expect_err("must reject");
		assert!(err.to_string().contains("missing unit"), "{err}");
	}

	#[test]
	fn renewal_duration_handles_h_d_units() {
		assert_eq!(parse_renewal_duration("30d").unwrap(), Duration::from_hours(720));
		assert_eq!(parse_renewal_duration("12h").unwrap(), Duration::from_hours(12));
		assert_eq!(parse_renewal_duration("90s").unwrap(), Duration::from_secs(90));
		assert_eq!(parse_renewal_duration("500ms").unwrap(), Duration::from_millis(500));
	}

	#[test]
	fn tls_managed_serializes_omitting_optional_fields() {
		let raw = managed_tls("http-01", false);
		let tls: TlsConfig = serde_json::from_value(raw).expect("parse");
		let json = serde_json::to_value(&tls).expect("serialize");
		// `cert_file` / `key_file` are skip_serializing_if=Option::is_none.
		assert!(json.as_object().expect("obj").get("cert_file").is_none());
		assert!(json.as_object().expect("obj").get("key_file").is_none());
		// `managed.dns_provider` likewise omitted when absent.
		assert!(json["managed"].as_object().expect("managed obj").get("dns_provider").is_none());
	}

	#[test]
	fn challenge_kind_round_trips_kebab_case() {
		assert_eq!(serde_json::to_string(&ChallengeKind::Http01).expect("ser"), "\"http-01\"");
		assert_eq!(serde_json::to_string(&ChallengeKind::Dns01).expect("ser"), "\"dns-01\"");
		let parsed: ChallengeKind = serde_json::from_str("\"http-01\"").expect("de");
		assert_eq!(parsed, ChallengeKind::Http01);
	}

	#[test]
	fn key_type_round_trips_kebab_case() {
		assert_eq!(serde_json::to_string(&ManagedKeyType::EcdsaP256).expect("ser"), "\"ecdsa-p256\"");
		assert_eq!(serde_json::to_string(&ManagedKeyType::Rsa2048).expect("ser"), "\"rsa-2048\"");
	}
}
