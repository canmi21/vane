//! Cloudflare DNS API [`super::DnsProvider`] implementation.
//!
//! Uses the Cloudflare v4 REST API for TXT-record CRUD. Token-based
//! auth via a Cloudflare API Token scoped to "Zone DNS Edit"; the
//! rule-side config carries only the env-var name that holds the
//! token, never the token itself, matching the
//! `09-config.md` `.env`-vs-config split.
//!
//! `wait_propagated` queries a small fixed pool of public recursive
//! resolvers (`1.1.1.1`, `8.8.8.8`) via `hickory-resolver`. Per
//! `spec/acme.md` § `wait_propagated` _semantics_, observing the
//! TXT through a public resolver is a high-confidence proxy for
//! what the CA validator will see.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use super::{DnsProvider, DnsProviderError};

/// Default Cloudflare API base. Overridden by tests via
/// [`CloudflareDnsProvider::with_api_base`] to point at a local
/// mock HTTP server.
const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";

/// Public recursive resolvers `wait_propagated` queries. Pebble's
/// validator uses similar resolvers in production; observing both
/// (or just one with retries) reduces the chance of seeing stale
/// negative caching.
const PUBLIC_RESOLVERS: &[(IpAddr, u16)] =
	&[(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53), (IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53)];

/// Operator-supplied Cloudflare config. Parsed from
/// `tls.managed.dns_provider` per `spec/acme.md`
/// § _Available providers_.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CloudflareConfig {
	/// Name of the environment variable holding the API token. The
	/// token itself never appears in the JSON config.
	pub api_token_env: String,
	/// Optional pre-configured zone id. When absent, the provider
	/// auto-detects the zone by walking the FQDN labels and
	/// querying `/zones?name=<apex>`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zone_id: Option<String>,
}

/// Cloudflare v4 API DNS provider.
///
/// Holds the resolved API token (from the env var) and a `reqwest`
/// client. `zone_id` is cached after the first lookup so subsequent
/// calls don't re-query `/zones`.
pub struct CloudflareDnsProvider {
	api_base: String,
	api_token: String,
	configured_zone_id: Option<String>,
	resolved_zone_id: Mutex<Option<String>>,
	http: Client,
}

impl std::fmt::Debug for CloudflareDnsProvider {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("CloudflareDnsProvider")
			.field("api_base", &self.api_base)
			.field("configured_zone_id", &self.configured_zone_id)
			.field("resolved_zone_id", &*self.resolved_zone_id.lock())
			.finish_non_exhaustive()
	}
}

impl CloudflareDnsProvider {
	/// Construct a provider from the operator config + env. Returns
	/// [`DnsProviderError::Auth`] when the configured env var is
	/// unset or empty.
	///
	/// # Errors
	///
	/// - [`DnsProviderError::Auth`] when the env var is missing /
	///   empty.
	/// - [`DnsProviderError::Internal`] when reqwest fails to build
	///   its client (typically a TLS-init issue).
	pub fn from_config(config: &CloudflareConfig) -> Result<Self, DnsProviderError> {
		let token = std::env::var(&config.api_token_env)
			.ok()
			.filter(|s| !s.is_empty())
			.ok_or(DnsProviderError::Auth)?;
		let http = Client::builder()
			.user_agent("vaned/acme")
			.build()
			.map_err(|e| DnsProviderError::Internal(format!("reqwest client: {e}")))?;
		Ok(Self {
			api_base: DEFAULT_API_BASE.to_owned(),
			api_token: token,
			configured_zone_id: config.zone_id.clone(),
			resolved_zone_id: Mutex::new(config.zone_id.clone()),
			http,
		})
	}

	fn auth_headers(&self) -> HeaderMap {
		let mut h = HeaderMap::new();
		let auth = format!("Bearer {}", self.api_token);
		h.insert(
			AUTHORIZATION,
			HeaderValue::from_str(&auth).unwrap_or_else(|_| HeaderValue::from_static("Bearer ")),
		);
		h.insert(ACCEPT, HeaderValue::from_static("application/json"));
		h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
		h
	}

	/// Resolve `name` to a zone id, using the configured / cached
	/// id first and falling back to a `/zones?name=<apex>` query.
	/// Cached on success.
	async fn resolve_zone_id(&self, name: &str) -> Result<String, DnsProviderError> {
		if let Some(id) = self.resolved_zone_id.lock().clone() {
			return Ok(id);
		}
		// Walk the labels right-to-left until we find a zone the
		// token can read. For most operators this is two-label
		// (`example.com`); some run subzones. The candidate set is
		// finite and bounded by the FQDN length.
		let mut candidates: Vec<String> = Vec::new();
		let labels: Vec<&str> = name.trim_end_matches('.').split('.').collect();
		for i in 0..labels.len().saturating_sub(1) {
			candidates.push(labels[i..].join("."));
		}
		for apex in candidates {
			if let Some(id) = self.lookup_zone_by_name(&apex).await? {
				*self.resolved_zone_id.lock() = Some(id.clone());
				return Ok(id);
			}
		}
		Err(DnsProviderError::ZoneNotFound(name.to_owned()))
	}

	async fn lookup_zone_by_name(&self, apex: &str) -> Result<Option<String>, DnsProviderError> {
		#[derive(Deserialize)]
		struct Zone {
			id: String,
		}
		#[derive(Deserialize)]
		struct ZonesResponse {
			result: Vec<Zone>,
			success: bool,
		}
		let resp = self
			.http
			.get(format!("{}/zones", self.api_base))
			.headers(self.auth_headers())
			.query(&[("name", apex)])
			.send()
			.await
			.map_err(|e| DnsProviderError::Api(format!("GET /zones: {e}")))?;
		match resp.status() {
			StatusCode::OK => {}
			StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(DnsProviderError::Auth),
			s => {
				return Err(DnsProviderError::Api(format!("GET /zones: status {s}")));
			}
		}
		let body: ZonesResponse =
			resp.json().await.map_err(|e| DnsProviderError::Api(format!("zones decode: {e}")))?;
		if !body.success {
			return Err(DnsProviderError::Api("/zones success=false".into()));
		}
		Ok(body.result.into_iter().next().map(|z| z.id))
	}

	async fn list_txt_record_ids(
		&self,
		zone_id: &str,
		name: &str,
	) -> Result<Vec<String>, DnsProviderError> {
		#[derive(Deserialize)]
		struct DnsRecord {
			id: String,
		}
		#[derive(Deserialize)]
		struct ListResponse {
			result: Vec<DnsRecord>,
			success: bool,
		}
		let resp = self
			.http
			.get(format!("{}/zones/{}/dns_records", self.api_base, zone_id))
			.headers(self.auth_headers())
			.query(&[("type", "TXT"), ("name", name)])
			.send()
			.await
			.map_err(|e| DnsProviderError::Api(format!("list dns_records: {e}")))?;
		match resp.status() {
			StatusCode::OK => {}
			StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(DnsProviderError::Auth),
			s => return Err(DnsProviderError::Api(format!("list dns_records: status {s}"))),
		}
		let body: ListResponse =
			resp.json().await.map_err(|e| DnsProviderError::Api(format!("dns_records decode: {e}")))?;
		if !body.success {
			return Err(DnsProviderError::Api("/dns_records success=false".into()));
		}
		Ok(body.result.into_iter().map(|r| r.id).collect())
	}

	async fn delete_record_by_id(
		&self,
		zone_id: &str,
		record_id: &str,
	) -> Result<(), DnsProviderError> {
		let resp = self
			.http
			.delete(format!("{}/zones/{}/dns_records/{}", self.api_base, zone_id, record_id))
			.headers(self.auth_headers())
			.send()
			.await
			.map_err(|e| DnsProviderError::Api(format!("delete dns_record: {e}")))?;
		match resp.status() {
			StatusCode::OK | StatusCode::NOT_FOUND => Ok(()),
			StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(DnsProviderError::Auth),
			s => Err(DnsProviderError::Api(format!("delete dns_record: status {s}"))),
		}
	}
}

#[derive(Serialize)]
struct CreateDnsRecord<'a> {
	r#type: &'static str,
	name: &'a str,
	content: &'a str,
	ttl: u32,
}

#[async_trait]
impl DnsProvider for CloudflareDnsProvider {
	async fn set_txt(&self, name: &str, value: &str) -> Result<(), DnsProviderError> {
		let zone_id = self.resolve_zone_id(name).await?;
		let resp = self
			.http
			.post(format!("{}/zones/{}/dns_records", self.api_base, zone_id))
			.headers(self.auth_headers())
			.json(&CreateDnsRecord {
				r#type: "TXT",
				name,
				content: value,
				// 60s — short enough that lingering records from a
				// failed earlier issuance fall out of caches before
				// the next attempt's wait_propagated runs.
				ttl: 60,
			})
			.send()
			.await
			.map_err(|e| DnsProviderError::Api(format!("create dns_record: {e}")))?;
		match resp.status() {
			StatusCode::OK | StatusCode::CREATED => Ok(()),
			StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(DnsProviderError::Auth),
			s => {
				let body = resp.text().await.unwrap_or_default();
				Err(DnsProviderError::Api(format!("create dns_record: status {s} body {body}")))
			}
		}
	}

	async fn delete_txt(&self, name: &str) -> Result<(), DnsProviderError> {
		let zone_id = self.resolve_zone_id(name).await?;
		let ids = self.list_txt_record_ids(&zone_id, name).await?;
		// Idempotent — empty `ids` list returns Ok without
		// touching the API again.
		for id in ids {
			self.delete_record_by_id(&zone_id, &id).await?;
		}
		Ok(())
	}

	async fn wait_propagated(
		&self,
		name: &str,
		value: &str,
		timeout: Duration,
	) -> Result<(), DnsProviderError> {
		let resolver = build_public_resolver();
		let deadline = Instant::now() + timeout;
		let expected = value.as_bytes();
		loop {
			if let Ok(lookup) = resolver.txt_lookup(name).await {
				let observed = lookup.answers().iter().any(|record| {
					if let hickory_resolver::proto::rr::RData::TXT(txt) = &record.data {
						txt.txt_data.iter().any(|d| d.as_ref() == expected)
					} else {
						false
					}
				});
				if observed {
					return Ok(());
				}
			}
			if Instant::now() >= deadline {
				return Err(DnsProviderError::PropagationTimeout(name.to_owned()));
			}
			// 500 ms cadence — public resolvers cache TXT for the
			// record's TTL (60s above), so faster polling burns
			// budget without changing the answer.
			tokio::time::sleep(Duration::from_millis(500)).await;
		}
	}
}

fn build_public_resolver() -> hickory_resolver::TokioResolver {
	use hickory_resolver::config::{
		ConnectionConfig, NameServerConfig, ResolverConfig, ResolverOpts,
	};
	use hickory_resolver::net::runtime::TokioRuntimeProvider;
	let name_servers: Vec<NameServerConfig> = PUBLIC_RESOLVERS
		.iter()
		.map(|(ip, port)| {
			let mut conn = ConnectionConfig::udp();
			conn.port = *port;
			NameServerConfig::new(*ip, true, vec![conn])
		})
		.collect();
	let cfg = ResolverConfig::from_parts(None, vec![], name_servers);
	let mut opts = ResolverOpts::default();
	opts.cache_size = 0;
	opts.attempts = 2;
	opts.timeout = Duration::from_secs(2);
	hickory_resolver::TokioResolver::builder_with_config(cfg, TokioRuntimeProvider::default())
		.with_options(opts)
		.build()
		.expect("public resolver builder")
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicU8, Ordering};

	use serde_json::json;
	use wiremock::matchers::{header, method, path, query_param};
	use wiremock::{Mock, MockServer, ResponseTemplate};

	use super::*;

	/// Sequence number so each "auth via env" test uses a unique
	/// env-var name and can't race other tests on the same
	/// process-wide env. The variable is intentionally never set
	/// — tests check the unset / empty path only, which doesn't
	/// require any unsafe `set_var`.
	static TOKEN_COUNTER: AtomicU8 = AtomicU8::new(0);

	fn unique_env_name() -> String {
		let n = TOKEN_COUNTER.fetch_add(1, Ordering::SeqCst);
		format!("VANE_TEST_CF_TOKEN_{n}")
	}

	fn build_provider_for_mock(
		server: &MockServer,
		zone_id: Option<String>,
	) -> CloudflareDnsProvider {
		// reqwest 0.12 with `rustls-tls-native-roots-no-provider`
		// requires the rustls crypto provider be installed before
		// `Client::build` constructs its (lazy) TLS config — even
		// for tests that only ever talk to a wiremock server over
		// plain HTTP, because reqwest builds the rustls config
		// eagerly. The engine installs aws-lc-rs at daemon boot;
		// tests must do the same.
		crate::crypto::install_default_provider();
		let http = Client::builder().user_agent("vaned/acme").build().expect("client");
		CloudflareDnsProvider {
			api_base: server.uri(),
			api_token: "test-token-123".to_owned(),
			configured_zone_id: zone_id.clone(),
			resolved_zone_id: Mutex::new(zone_id),
			http,
		}
	}

	#[tokio::test]
	async fn from_config_returns_auth_when_env_unset() {
		let env_name = unique_env_name();
		let cfg = CloudflareConfig { api_token_env: env_name, zone_id: None };
		match CloudflareDnsProvider::from_config(&cfg) {
			Err(DnsProviderError::Auth) => {}
			other => panic!("expected Auth, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn lookup_zone_by_name_returns_first_match() {
		let server = MockServer::start().await;
		Mock::given(method("GET"))
			.and(path("/zones"))
			.and(query_param("name", "example.com"))
			.and(header("authorization", "Bearer test-token-123"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true,
				"result": [{"id": "zone-id-abc"}],
			})))
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, None);
		let id = provider.lookup_zone_by_name("example.com").await.expect("zone").expect("Some");
		assert_eq!(id, "zone-id-abc");
	}

	#[tokio::test]
	async fn lookup_zone_by_name_returns_none_when_no_match() {
		let server = MockServer::start().await;
		Mock::given(method("GET"))
			.and(path("/zones"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true,
				"result": [],
			})))
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, None);
		let id = provider.lookup_zone_by_name("missing.example").await.expect("ok");
		assert!(id.is_none());
	}

	#[tokio::test]
	async fn lookup_zone_returns_auth_on_401() {
		let server = MockServer::start().await;
		Mock::given(method("GET"))
			.and(path("/zones"))
			.respond_with(ResponseTemplate::new(401))
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, None);
		match provider.lookup_zone_by_name("example.com").await {
			Err(DnsProviderError::Auth) => {}
			other => panic!("expected Auth, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn set_txt_posts_record_with_correct_shape() {
		let server = MockServer::start().await;
		// The provider is pre-configured with zone_id so it skips
		// the /zones lookup and goes straight to POST.
		Mock::given(method("POST"))
			.and(path("/zones/zone-id-abc/dns_records"))
			.and(header("authorization", "Bearer test-token-123"))
			.and(header("content-type", "application/json"))
			.and(wiremock::matchers::body_json(json!({
				"type": "TXT",
				"name": "_acme-challenge.example.com",
				"content": "ka-VALUE",
				"ttl": 60,
			})))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true,
				"result": {"id": "rec-id-1"},
			})))
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, Some("zone-id-abc".into()));
		provider.set_txt("_acme-challenge.example.com", "ka-VALUE").await.expect("set_txt ok");
	}

	#[tokio::test]
	async fn delete_txt_idempotent_when_no_records() {
		let server = MockServer::start().await;
		Mock::given(method("GET"))
			.and(path("/zones/zone-id-abc/dns_records"))
			.and(query_param("type", "TXT"))
			.and(query_param("name", "_acme-challenge.example.com"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true,
				"result": [],
			})))
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, Some("zone-id-abc".into()));
		provider.delete_txt("_acme-challenge.example.com").await.expect("idempotent delete");
	}

	#[tokio::test]
	async fn delete_txt_calls_delete_for_each_match() {
		let server = MockServer::start().await;
		Mock::given(method("GET"))
			.and(path("/zones/zone-id-abc/dns_records"))
			.and(query_param("type", "TXT"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true,
				"result": [{"id": "rec-1"}, {"id": "rec-2"}],
			})))
			.mount(&server)
			.await;
		Mock::given(method("DELETE"))
			.and(path("/zones/zone-id-abc/dns_records/rec-1"))
			.respond_with(ResponseTemplate::new(200))
			.expect(1)
			.mount(&server)
			.await;
		Mock::given(method("DELETE"))
			.and(path("/zones/zone-id-abc/dns_records/rec-2"))
			.respond_with(ResponseTemplate::new(200))
			.expect(1)
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, Some("zone-id-abc".into()));
		provider.delete_txt("_acme-challenge.example.com").await.expect("delete ok");
	}

	#[tokio::test]
	async fn resolve_zone_id_walks_labels_until_match() {
		// Provider has no pre-configured zone_id; the auto-detect
		// path walks `_acme-challenge.api.example.com` → `api.example.com`
		// → `example.com`. Mock responds 200/empty for the longer
		// names, 200/match for `example.com`.
		let server = MockServer::start().await;
		Mock::given(method("GET"))
			.and(path("/zones"))
			.and(query_param("name", "_acme-challenge.api.example.com"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true, "result": [],
			})))
			.mount(&server)
			.await;
		Mock::given(method("GET"))
			.and(path("/zones"))
			.and(query_param("name", "api.example.com"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true, "result": [],
			})))
			.mount(&server)
			.await;
		Mock::given(method("GET"))
			.and(path("/zones"))
			.and(query_param("name", "example.com"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true, "result": [{"id": "zone-id-walked"}],
			})))
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, None);
		let id = provider.resolve_zone_id("_acme-challenge.api.example.com").await.expect("resolved");
		assert_eq!(id, "zone-id-walked");
	}

	#[tokio::test]
	async fn resolve_zone_id_caches_after_lookup() {
		let server = MockServer::start().await;
		Mock::given(method("GET"))
			.and(path("/zones"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true, "result": [{"id": "zone-id-cached"}],
			})))
			.expect(1) // Exactly one /zones call across both invocations.
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, None);
		let _ = provider.resolve_zone_id("api.example.com").await.expect("first");
		let id2 = provider.resolve_zone_id("api.example.com").await.expect("second");
		assert_eq!(id2, "zone-id-cached");
	}

	#[tokio::test]
	async fn resolve_zone_id_returns_zone_not_found_when_no_apex_matches() {
		let server = MockServer::start().await;
		Mock::given(method("GET"))
			.and(path("/zones"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({
				"success": true, "result": [],
			})))
			.mount(&server)
			.await;
		let provider = build_provider_for_mock(&server, None);
		match provider.resolve_zone_id("anything.unknown.example").await {
			Err(DnsProviderError::ZoneNotFound(_)) => {}
			other => panic!("expected ZoneNotFound, got {other:?}"),
		}
	}
}
