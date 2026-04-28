//! `rate_limit` — token-bucket L7 stateful middleware.
//!
//! Per-IP or global key, configurable rate / burst / window. On
//! exhaustion, returns `Decision::Short(ShortCircuit::Response(_))`
//! with a configurable `on_limit` payload (default 429). The wire-level
//! 429 only appears once the executor's
//! `meta.short_circuit_response_entry` routing is in place; until that
//! lands the middleware itself is correct but the daemon emits 500.
//!
//! See `spec/architecture/13-rate-limit.md` § _L2 — User
//! application-layer rate limiting_, and
//! `spec/architecture/04-middleware.md` § _Stateful internal_ —
//! `rate_limit` is the canonical example of a per-call-site stateful
//! middleware. The metadata provider must mark `stateless: false` so
//! `lower::intern_middleware` skips dedup; two rules each declaring
//! their own `rate_limit` get separate buckets, never a shared one.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use bytes::Bytes;
use dashmap::DashMap;
use http::{HeaderName, HeaderValue};
use parking_lot::Mutex;
use vane_core::{
	Body, ConnContext, Decision, Error, FlowCtx, L7RequestMiddleware, MiddlewareKind, Request,
	Response, ShortCircuit,
};

use crate::factories::{FactoryError, MiddlewareFactories};
use crate::flow_graph::MiddlewareInst;

/// Spec `13-rate-limit.md § _L2_` constrains the window range to `[1s, 60s]`.
const MIN_WINDOW_SECS: u64 = 1;
const MAX_WINDOW_SECS: u64 = 60;

/// Subset of spec § _Key derivation_ that this round implements.
/// Header / Cookie / Query / Composite are post-MVP — they need a
/// request-field reader helper that doesn't exist yet.
#[derive(Debug, Clone, Copy)]
enum KeyDerivation {
	RemoteIp,
	Global,
}

#[derive(Hash, Eq, PartialEq, Clone)]
enum BucketKey {
	RemoteIp(IpAddr),
	Global,
}

struct TokenBucket {
	state: Mutex<TokenState>,
}

struct TokenState {
	tokens: f64,
	last_refill: Instant,
}

impl TokenBucket {
	fn new(burst: u32) -> Self {
		Self { state: Mutex::new(TokenState { tokens: f64::from(burst), last_refill: Instant::now() }) }
	}

	/// Returns `true` if a token was consumed (request allowed).
	fn try_consume(&self, rate_per_sec: f64, capacity: u32) -> bool {
		let mut s = self.state.lock();
		let now = Instant::now();
		let elapsed = now.duration_since(s.last_refill).as_secs_f64();
		s.tokens = (s.tokens + elapsed * rate_per_sec).min(f64::from(capacity));
		s.last_refill = now;
		if s.tokens >= 1.0 {
			s.tokens -= 1.0;
			true
		} else {
			false
		}
	}
}

pub struct RateLimitMiddleware {
	buckets: DashMap<BucketKey, TokenBucket>,
	rate_per_sec: f64,
	capacity: u32,
	key_derivation: KeyDerivation,
	response_status: u16,
	response_headers: Vec<(HeaderName, HeaderValue)>,
	response_body: Bytes,
}

#[async_trait]
impl L7RequestMiddleware for RateLimitMiddleware {
	async fn run(
		&self,
		_req: &mut Request,
		conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let key = match self.key_derivation {
			KeyDerivation::RemoteIp => BucketKey::RemoteIp(conn.remote.ip()),
			KeyDerivation::Global => BucketKey::Global,
		};
		let allowed = {
			let bucket = self.buckets.entry(key).or_insert_with(|| TokenBucket::new(self.capacity));
			bucket.try_consume(self.rate_per_sec, self.capacity)
		};
		if allowed {
			Ok(Decision::Continue)
		} else {
			let (limit_label, source_label): (&'static str, String) = match self.key_derivation {
				KeyDerivation::RemoteIp => ("per_ip", conn.remote.ip().to_string()),
				KeyDerivation::Global => ("global", "global".to_string()),
			};
			metrics::counter!(
				"vane.security.limit_hit_total",
				"limit" => limit_label,
				"source" => source_label,
			)
			.increment(1);
			let body = if self.response_body.is_empty() {
				Body::Empty
			} else {
				Body::Static(self.response_body.clone())
			};
			let mut builder = http::Response::builder().status(self.response_status);
			for (name, value) in &self.response_headers {
				builder = builder.header(name, value);
			}
			let resp: Response =
				builder.body(body).map_err(|e| Error::internal(format!("rate_limit reject build: {e}")))?;
			Ok(Decision::Short(ShortCircuit::Response(resp)))
		}
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape:
///
/// ```json
/// {
///   "key":      "remote_ip" | "global",
///   "rate":     <u32 tokens per window>,
///   "burst":    <u32 bucket capacity, ≥ 1>,
///   "window":   "<N>s"  (1..=60),
///   "on_limit": { "status": 429, "headers": { ... }, "body": "<base64>" }
/// }
/// ```
///
/// Defaults: `key = "remote_ip"`, `on_limit = { status: 429, headers:
/// {}, body: "" }`. `rate`, `burst`, `window` are required.
///
/// # Errors
/// Returns [`FactoryError`] for any of: missing `rate` / `burst` /
/// `window`, `burst == 0`, malformed `window`, unsupported `key`,
/// invalid `on_limit.status` (out of u16 or HTTP range), unrecognised
/// header name, non-string header value, or malformed `on_limit.body`
/// base64.
pub fn factory(args: &serde_json::Value) -> Result<MiddlewareInst, FactoryError> {
	let rate = args
		.get("rate")
		.and_then(serde_json::Value::as_u64)
		.ok_or_else(|| FactoryError("missing args.rate (u32 tokens-per-window)".to_string()))?;
	let rate = u32::try_from(rate).map_err(|_| FactoryError("args.rate exceeds u32".to_string()))?;

	let burst = args
		.get("burst")
		.and_then(serde_json::Value::as_u64)
		.ok_or_else(|| FactoryError("missing args.burst (u32 bucket capacity)".to_string()))?;
	let burst =
		u32::try_from(burst).map_err(|_| FactoryError("args.burst exceeds u32".to_string()))?;
	if burst == 0 {
		return Err(FactoryError("args.burst must be ≥ 1".to_string()));
	}

	let window_str = args
		.get("window")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| FactoryError("missing args.window (\"Ns\" with N in 1..=60)".to_string()))?;
	let window = parse_window(window_str)?;

	let key_str = args.get("key").and_then(serde_json::Value::as_str).unwrap_or("remote_ip");
	let key_derivation = match key_str {
		"remote_ip" => KeyDerivation::RemoteIp,
		"global" => KeyDerivation::Global,
		other => {
			return Err(FactoryError(format!(
				"unsupported args.key {other:?}; supported: remote_ip / global \
				 (header / cookie / query / composite are post-MVP per \
				 spec/architecture/13-rate-limit.md § _Key derivation_)"
			)));
		}
	};

	let (status, headers, body) = parse_on_limit(args.get("on_limit"))?;

	let rate_per_sec = f64::from(rate) / window.as_secs_f64();

	Ok(MiddlewareInst::L7Request(Arc::new(RateLimitMiddleware {
		buckets: DashMap::new(),
		rate_per_sec,
		capacity: burst,
		key_derivation,
		response_status: status,
		response_headers: headers,
		response_body: body,
	})))
}

fn parse_window(s: &str) -> Result<Duration, FactoryError> {
	let trimmed = s
		.strip_suffix('s')
		.ok_or_else(|| FactoryError(format!("args.window {s:?}: must end with 's' (e.g. \"1s\")")))?;
	let n: u64 = trimmed.parse().map_err(|e| FactoryError(format!("args.window {s:?}: {e}")))?;
	if !(MIN_WINDOW_SECS..=MAX_WINDOW_SECS).contains(&n) {
		return Err(FactoryError(format!(
			"args.window {s:?}: must be in [1s, 60s] per spec/architecture/13-rate-limit.md"
		)));
	}
	Ok(Duration::from_secs(n))
}

type OnLimit = (u16, Vec<(HeaderName, HeaderValue)>, Bytes);

fn parse_on_limit(v: Option<&serde_json::Value>) -> Result<OnLimit, FactoryError> {
	let Some(obj) = v.and_then(|v| v.as_object()) else {
		return Ok((429, vec![], Bytes::new()));
	};
	let status_raw = obj.get("status").and_then(serde_json::Value::as_u64).unwrap_or(429);
	let status = u16::try_from(status_raw)
		.map_err(|_| FactoryError(format!("on_limit.status {status_raw} out of u16 range")))?;
	if !(100..=599).contains(&status) {
		return Err(FactoryError(format!("on_limit.status {status} out of HTTP range 100-599")));
	}

	let mut headers = Vec::new();
	if let Some(hdrs) = obj.get("headers").and_then(serde_json::Value::as_object) {
		for (k, v) in hdrs {
			let name = HeaderName::try_from(k.as_str())
				.map_err(|e| FactoryError(format!("on_limit.headers name {k:?}: {e}")))?;
			let s = v
				.as_str()
				.ok_or_else(|| FactoryError(format!("on_limit.headers[{k:?}] must be string")))?;
			let value = HeaderValue::try_from(s)
				.map_err(|e| FactoryError(format!("on_limit.headers[{k:?}] value: {e}")))?;
			headers.push((name, value));
		}
	}

	let body = if let Some(b64) = obj.get("body").and_then(serde_json::Value::as_str) {
		Bytes::from(
			BASE64_STANDARD
				.decode(b64.as_bytes())
				.map_err(|e| FactoryError(format!("on_limit.body base64: {e}")))?,
		)
	} else {
		Bytes::new()
	};

	Ok((status, headers, body))
}

/// Plug `rate_limit` into a `MiddlewareFactories` registry.
pub fn register(factories: &mut MiddlewareFactories) {
	factories.register("rate_limit", MiddlewareKind::L7Request, factory);
}
