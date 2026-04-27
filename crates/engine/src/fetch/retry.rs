//! Retry policy for [`crate::fetch::http_proxy::HttpProxyFetch`].
//!
//! Configuration shape (per `spec/architecture/05-terminator.md`
//! § _Retry_):
//!
//! ```json
//! {
//!   "max_attempts": 3,
//!   "methods":      ["GET", "HEAD", "PUT", "DELETE", "OPTIONS"],
//!   "backoff":      "exponential",
//!   "buffering":    "opportunistic"
//! }
//! ```
//!
//! All four fields are optional. The retry decision itself goes
//! through [`vane_core::Error::is_retryable`] — the spec's
//! single-source error-classification table — so this module owns
//! the *policy* (how many attempts, when, on which methods) and
//! [`vane_core::Error::is_retryable`] owns the *eligibility table*.

use std::collections::HashSet;
use std::time::Duration;

use http::Method;
use rand::RngExt;

#[derive(Clone, Debug)]
pub struct RetryPolicy {
	/// Total attempts including the first try. `1` disables retry.
	pub max_attempts: u32,
	/// HTTP methods that may retry. Defaults to the RFC 7231
	/// idempotent set (GET / HEAD / PUT / DELETE / OPTIONS); POST
	/// and PATCH require explicit opt-in.
	pub methods: HashSet<Method>,
	pub backoff: Backoff,
	pub buffering: BufferingPolicy,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BufferingPolicy {
	/// Default: retry only when the body is already buffered. A
	/// `Body::Stream` request collapses retry to a single attempt.
	Opportunistic,
	/// Lower flags the fetch's incoming edge with
	/// `collect_body_before: Some(BodySide::Request)` so the body
	/// always arrives as `Body::Static`. Predictable retry,
	/// deterministic memory cost.
	Force,
}

#[derive(Clone, Debug)]
pub enum Backoff {
	None,
	Fixed(Duration),
	Exponential { base: Duration, max: Duration, jitter: bool },
}

impl Default for RetryPolicy {
	fn default() -> Self {
		Self {
			max_attempts: 1,
			methods: default_idempotent_methods(),
			backoff: default_exponential(),
			buffering: BufferingPolicy::Opportunistic,
		}
	}
}

fn default_idempotent_methods() -> HashSet<Method> {
	[Method::GET, Method::HEAD, Method::PUT, Method::DELETE, Method::OPTIONS].into_iter().collect()
}

fn default_exponential() -> Backoff {
	Backoff::Exponential {
		base: Duration::from_millis(100),
		max: Duration::from_secs(5),
		jitter: true,
	}
}

impl Backoff {
	/// Sleep duration *before* `attempt`. `attempt` is 1-indexed and
	/// counts from the original request: `attempt == 1` is the
	/// first try (no pre-sleep), `attempt == 2` is the first retry,
	/// etc. The exponential formula is `base * 2^(attempt - 2)`,
	/// capped at `max`; full jitter multiplies by a uniform
	/// `[0, 1)` factor.
	#[must_use]
	pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
		if attempt <= 1 {
			return Duration::ZERO;
		}
		match self {
			Self::None => Duration::ZERO,
			Self::Fixed(d) => *d,
			Self::Exponential { base, max, jitter } => {
				let n = attempt.saturating_sub(2);
				// Cap the shift at 20 so `2^n` never overflows `u32`.
				let shift = n.min(20);
				let exp = base.saturating_mul(1u32 << shift);
				let capped = exp.min(*max);
				if *jitter {
					let factor: f64 = rand::rng().random_range(0.0..1.0);
					capped.mul_f64(factor)
				} else {
					capped
				}
			}
		}
	}
}

/// Parse `args.retry` into a `RetryPolicy`. Missing / null / empty
/// object yields the default (no retry).
///
/// # Errors
/// String description of any schema violation: bad type, unknown
/// `buffering` value, unparseable `backoff`, invalid HTTP method
/// name.
pub fn parse(retry: Option<&serde_json::Value>) -> Result<RetryPolicy, String> {
	let Some(retry) = retry else {
		return Ok(RetryPolicy::default());
	};
	if retry.is_null() {
		return Ok(RetryPolicy::default());
	}

	let mut policy = RetryPolicy::default();

	if let Some(m) = retry.get("max_attempts") {
		let n = m.as_u64().ok_or("max_attempts must be a positive integer")?;
		if n == 0 {
			return Err("max_attempts must be >= 1".to_owned());
		}
		policy.max_attempts = u32::try_from(n).map_err(|_| "max_attempts too large".to_owned())?;
	}

	if let Some(methods) = retry.get("methods") {
		let arr = methods.as_array().ok_or("methods must be an array of strings")?;
		let mut set = HashSet::new();
		for m in arr {
			let s = m.as_str().ok_or("methods entries must be strings")?;
			let parsed =
				Method::from_bytes(s.as_bytes()).map_err(|e| format!("invalid method {s:?}: {e}"))?;
			set.insert(parsed);
		}
		policy.methods = set;
	}

	if let Some(b) = retry.get("backoff") {
		policy.backoff = parse_backoff(b)?;
	}

	if let Some(buf) = retry.get("buffering") {
		let s = buf.as_str().ok_or("buffering must be a string")?;
		policy.buffering = match s {
			"opportunistic" => BufferingPolicy::Opportunistic,
			"force" => BufferingPolicy::Force,
			other => {
				return Err(format!("buffering must be 'opportunistic' or 'force', got {other:?}"));
			}
		};
	}

	Ok(policy)
}

fn parse_backoff(v: &serde_json::Value) -> Result<Backoff, String> {
	if let Some(s) = v.as_str() {
		return match s {
			"none" => Ok(Backoff::None),
			"exponential" => Ok(default_exponential()),
			other => Err(format!("backoff string must be 'none' or 'exponential', got {other:?}")),
		};
	}
	let obj = v.as_object().ok_or("backoff must be a string or object")?;
	if let Some(fixed) = obj.get("fixed") {
		let s = fixed.as_str().ok_or("backoff.fixed must be a duration string")?;
		return Ok(Backoff::Fixed(parse_duration(s)?));
	}
	if let Some(exp) = obj.get("exponential") {
		let exp = exp.as_object().ok_or("backoff.exponential must be an object")?;
		let base = exp
			.get("base")
			.and_then(serde_json::Value::as_str)
			.map(parse_duration)
			.transpose()?
			.unwrap_or(Duration::from_millis(100));
		let max = exp
			.get("max")
			.and_then(serde_json::Value::as_str)
			.map(parse_duration)
			.transpose()?
			.unwrap_or(Duration::from_secs(5));
		let jitter = exp.get("jitter").and_then(serde_json::Value::as_bool).unwrap_or(true);
		return Ok(Backoff::Exponential { base, max, jitter });
	}
	Err("backoff object must have 'fixed' or 'exponential' key".to_owned())
}

/// Parse a duration literal of the form `<integer><unit>` where
/// `unit ∈ { "ms", "s", "m" }`. Permissive enough for retry
/// backoff configs without pulling in `humantime`.
fn parse_duration(s: &str) -> Result<Duration, String> {
	let s = s.trim();
	let (num, unit) = if let Some(stripped) = s.strip_suffix("ms") {
		(stripped, "ms")
	} else if let Some(stripped) = s.strip_suffix('s') {
		(stripped, "s")
	} else if let Some(stripped) = s.strip_suffix('m') {
		(stripped, "m")
	} else {
		return Err(format!("duration {s:?}: missing unit (expected ms / s / m)"));
	};
	let n: u64 = num.parse().map_err(|e| format!("duration {s:?}: {e}"))?;
	Ok(match unit {
		"ms" => Duration::from_millis(n),
		"s" => Duration::from_secs(n),
		"m" => Duration::from_secs(n * 60),
		_ => unreachable!(),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn parse_returns_default_when_field_absent() {
		let p = parse(None).expect("none");
		assert_eq!(p.max_attempts, 1);
	}

	#[test]
	fn parse_returns_default_when_field_null() {
		let p = parse(Some(&serde_json::Value::Null)).expect("null");
		assert_eq!(p.max_attempts, 1);
	}

	#[test]
	fn parse_default_methods_are_idempotent_set() {
		let p = parse(None).expect("none");
		assert!(p.methods.contains(&Method::GET));
		assert!(p.methods.contains(&Method::HEAD));
		assert!(p.methods.contains(&Method::PUT));
		assert!(p.methods.contains(&Method::DELETE));
		assert!(p.methods.contains(&Method::OPTIONS));
		assert!(!p.methods.contains(&Method::POST));
		assert!(!p.methods.contains(&Method::PATCH));
	}

	#[test]
	fn parse_default_backoff_is_exponential_with_jitter() {
		let p = parse(None).expect("none");
		match p.backoff {
			Backoff::Exponential { base, max, jitter } => {
				assert_eq!(base, Duration::from_millis(100));
				assert_eq!(max, Duration::from_secs(5));
				assert!(jitter);
			}
			other => panic!("expected exponential, got {other:?}"),
		}
	}

	#[test]
	fn parse_default_buffering_is_opportunistic() {
		let p = parse(None).expect("none");
		assert_eq!(p.buffering, BufferingPolicy::Opportunistic);
	}

	#[test]
	fn parse_explicit_max_attempts() {
		let p = parse(Some(&json!({ "max_attempts": 5 }))).expect("ok");
		assert_eq!(p.max_attempts, 5);
	}

	#[test]
	fn parse_methods_post_can_opt_in() {
		let p = parse(Some(&json!({ "methods": ["POST", "GET"] }))).expect("ok");
		assert!(p.methods.contains(&Method::POST));
		assert!(p.methods.contains(&Method::GET));
		assert!(!p.methods.contains(&Method::HEAD), "explicit set replaces default");
	}

	#[test]
	fn parse_backoff_string_none() {
		let p = parse(Some(&json!({ "backoff": "none" }))).expect("ok");
		assert!(matches!(p.backoff, Backoff::None));
	}

	#[test]
	fn parse_backoff_string_exponential() {
		let p = parse(Some(&json!({ "backoff": "exponential" }))).expect("ok");
		assert!(matches!(p.backoff, Backoff::Exponential { .. }));
	}

	#[test]
	fn parse_backoff_object_fixed() {
		let p = parse(Some(&json!({ "backoff": { "fixed": "250ms" } }))).expect("ok");
		match p.backoff {
			Backoff::Fixed(d) => assert_eq!(d, Duration::from_millis(250)),
			other => panic!("expected fixed, got {other:?}"),
		}
	}

	#[test]
	fn parse_backoff_object_exponential_explicit_params() {
		let p = parse(Some(&json!({
			"backoff": { "exponential": { "base": "50ms", "max": "1s", "jitter": false } }
		})))
		.expect("ok");
		match p.backoff {
			Backoff::Exponential { base, max, jitter } => {
				assert_eq!(base, Duration::from_millis(50));
				assert_eq!(max, Duration::from_secs(1));
				assert!(!jitter);
			}
			other => panic!("expected exponential, got {other:?}"),
		}
	}

	#[test]
	fn parse_buffering_force() {
		let p = parse(Some(&json!({ "buffering": "force" }))).expect("ok");
		assert_eq!(p.buffering, BufferingPolicy::Force);
	}

	#[test]
	fn parse_buffering_opportunistic_explicit() {
		let p = parse(Some(&json!({ "buffering": "opportunistic" }))).expect("ok");
		assert_eq!(p.buffering, BufferingPolicy::Opportunistic);
	}

	#[test]
	fn parse_rejects_max_attempts_zero() {
		let err = parse(Some(&json!({ "max_attempts": 0 }))).expect_err("zero rejected");
		assert!(err.contains(">= 1"), "{err}");
	}

	#[test]
	fn parse_rejects_unknown_buffering() {
		let err = parse(Some(&json!({ "buffering": "weird" }))).expect_err("unknown buffering");
		assert!(err.contains("opportunistic") && err.contains("force"), "{err}");
	}

	#[test]
	fn parse_rejects_invalid_method_string() {
		let err = parse(Some(&json!({ "methods": ["NOT A METHOD"] }))).expect_err("bad method");
		assert!(err.contains("invalid method"), "{err}");
	}

	#[test]
	fn backoff_none_returns_zero() {
		assert_eq!(Backoff::None.delay_for_attempt(2), Duration::ZERO);
		assert_eq!(Backoff::None.delay_for_attempt(10), Duration::ZERO);
	}

	#[test]
	fn backoff_first_attempt_is_zero_for_every_kind() {
		assert_eq!(Backoff::None.delay_for_attempt(1), Duration::ZERO);
		assert_eq!(Backoff::Fixed(Duration::from_millis(100)).delay_for_attempt(1), Duration::ZERO);
		assert_eq!(default_exponential().delay_for_attempt(1), Duration::ZERO);
	}

	#[test]
	fn backoff_fixed_returns_configured_duration() {
		let b = Backoff::Fixed(Duration::from_millis(200));
		assert_eq!(b.delay_for_attempt(2), Duration::from_millis(200));
		assert_eq!(b.delay_for_attempt(5), Duration::from_millis(200));
	}

	#[test]
	fn backoff_exponential_grows_until_max() {
		let b = Backoff::Exponential {
			base: Duration::from_millis(10),
			max: Duration::from_millis(80),
			jitter: false,
		};
		assert_eq!(b.delay_for_attempt(2), Duration::from_millis(10));
		assert_eq!(b.delay_for_attempt(3), Duration::from_millis(20));
		assert_eq!(b.delay_for_attempt(4), Duration::from_millis(40));
		assert_eq!(b.delay_for_attempt(5), Duration::from_millis(80));
		assert_eq!(b.delay_for_attempt(10), Duration::from_millis(80), "capped");
	}

	#[test]
	fn backoff_exponential_with_jitter_in_range() {
		let b = Backoff::Exponential {
			base: Duration::from_millis(100),
			max: Duration::from_secs(1),
			jitter: true,
		};
		// Run several samples; full-jitter must stay in [0, base * 2^(attempt-2)]
		// for the smaller attempts, capped at max.
		for _ in 0..32 {
			let d = b.delay_for_attempt(2);
			assert!(d <= Duration::from_millis(100), "{d:?}");
		}
		for _ in 0..32 {
			let d = b.delay_for_attempt(10);
			assert!(d <= Duration::from_secs(1), "{d:?}");
		}
	}

	#[test]
	fn parse_duration_handles_ms_s_m() {
		assert_eq!(parse_duration("100ms").unwrap(), Duration::from_millis(100));
		assert_eq!(parse_duration("5s").unwrap(), Duration::from_secs(5));
		assert_eq!(parse_duration("2m").unwrap(), Duration::from_mins(2));
	}

	#[test]
	fn parse_duration_rejects_missing_unit() {
		let err = parse_duration("100").expect_err("missing unit");
		assert!(err.contains("unit"), "{err}");
	}
}
