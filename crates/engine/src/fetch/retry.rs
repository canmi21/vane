//! Retry policy for [`crate::fetch::http_proxy::HttpProxyFetch`].
//!
//! Configuration shape (per `spec/crates/engine.md`
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
//!
//! The policy types and `parse_duration` helper live in
//! [`http_retry_policy`]; this module re-exports them and adds the
//! vane-specific JSON schema parser.

use std::collections::HashSet;
use std::time::Duration;

use http::Method;

pub use http_retry_policy::{Backoff, BufferingPolicy, RetryPolicy, parse_duration};

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
			"exponential" => Ok(Backoff::exponential_default()),
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
}
