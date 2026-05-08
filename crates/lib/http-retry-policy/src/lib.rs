//! A serializable HTTP retry policy: `Backoff` enum, idempotent-method
//! gating, and explicit body-buffering semantics. Sits in the gap
//! between `tower-retry` (no backoff, no method gating) and `backon`
//! (backoff but no HTTP-aware policy types).
//!
//! See the README for the gap this fills and the JSON schema callers
//! typically wrap this in.

use std::collections::HashSet;
use std::time::Duration;

use http::Method;
use rand::RngExt;

/// HTTP retry policy. Pair with a per-attempt loop that consults
/// [`Backoff::delay_for_attempt`] and the configured method allow-list.
#[derive(Clone, Debug)]
pub struct RetryPolicy {
	/// Total attempts including the first try. `1` disables retry.
	pub max_attempts: u32,
	/// HTTP methods that may retry. Defaults to the RFC 9110 idempotent
	/// set (GET / HEAD / PUT / DELETE / OPTIONS); POST and PATCH
	/// require explicit opt-in.
	pub methods: HashSet<Method>,
	pub backoff: Backoff,
	pub buffering: BufferingPolicy,
}

/// Body-buffering posture for retry. Names the trade-off between
/// "retries are safe for any body" and "request body memory cost is
/// bounded".
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BufferingPolicy {
	/// Default: retry only when the body is already buffered. A
	/// streaming request body collapses retry to a single attempt.
	Opportunistic,
	/// Force the body to be fully buffered up-front so retries are
	/// always safe. Predictable retry, deterministic memory cost.
	Force,
}

/// Backoff strategy between retry attempts.
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
			methods: Self::idempotent_methods(),
			backoff: Backoff::exponential_default(),
			buffering: BufferingPolicy::Opportunistic,
		}
	}
}

impl RetryPolicy {
	/// The RFC 9110 idempotent method set: GET, HEAD, PUT, DELETE,
	/// OPTIONS. Used as the default `methods` allow-list.
	#[must_use]
	pub fn idempotent_methods() -> HashSet<Method> {
		[Method::GET, Method::HEAD, Method::PUT, Method::DELETE, Method::OPTIONS].into_iter().collect()
	}
}

impl Backoff {
	/// Default exponential backoff: 100 ms base, 5 s cap, full jitter.
	#[must_use]
	pub fn exponential_default() -> Self {
		Self::Exponential {
			base: Duration::from_millis(100),
			max: Duration::from_secs(5),
			jitter: true,
		}
	}

	/// Sleep duration *before* `attempt`. `attempt` is 1-indexed and
	/// counts from the original request: `attempt == 1` is the first
	/// try (no pre-sleep), `attempt == 2` is the first retry, etc.
	/// The exponential formula is `base * 2^(attempt - 2)`, capped at
	/// `max`; full jitter multiplies by a uniform `[0, 1)` factor.
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

/// Parse a duration literal of the form `<integer><unit>` where
/// `unit ∈ { "ms", "s", "m" }`. Permissive enough for retry backoff
/// configs without pulling in `humantime`.
///
/// # Errors
/// Returns a string description of the parse failure: missing unit
/// suffix, unparseable integer, or unrecognised unit.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
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

	#[test]
	fn default_policy_is_no_retry() {
		let p = RetryPolicy::default();
		assert_eq!(p.max_attempts, 1);
	}

	#[test]
	fn default_methods_are_idempotent_set() {
		let p = RetryPolicy::default();
		assert!(p.methods.contains(&Method::GET));
		assert!(p.methods.contains(&Method::HEAD));
		assert!(p.methods.contains(&Method::PUT));
		assert!(p.methods.contains(&Method::DELETE));
		assert!(p.methods.contains(&Method::OPTIONS));
		assert!(!p.methods.contains(&Method::POST));
		assert!(!p.methods.contains(&Method::PATCH));
	}

	#[test]
	fn default_backoff_is_exponential_with_jitter() {
		let p = RetryPolicy::default();
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
	fn default_buffering_is_opportunistic() {
		let p = RetryPolicy::default();
		assert_eq!(p.buffering, BufferingPolicy::Opportunistic);
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
		assert_eq!(Backoff::exponential_default().delay_for_attempt(1), Duration::ZERO);
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
