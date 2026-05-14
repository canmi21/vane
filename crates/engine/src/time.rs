//! Crate-internal time helpers. Mostly the wall-clock-to-unix-ms
//! conversion, which used to be copy-pasted in five files with three
//! subtly different "before epoch" semantics.

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock `SystemTime` → milliseconds since the Unix epoch.
///
/// Pre-1970 timestamps clamp to `0`. Post-`u64::MAX` ms (year 584
/// million-ish) saturates at `u64::MAX`. Both edge cases are
/// physically unreachable on a real machine, but the explicit
/// saturation keeps the conversion total — callers can always
/// destructure the `u64` without an `expect`.
pub(crate) fn system_time_to_unix_ms(t: SystemTime) -> u64 {
	t.duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Current wall-clock time in milliseconds since the Unix epoch.
///
/// Convenience over [`system_time_to_unix_ms`]`(SystemTime::now())`.
/// Every site that stamps a flow log event, listener id, or
/// request-arrival timestamp uses this — keep it cheap and total.
pub(crate) fn now_unix_ms() -> u64 {
	system_time_to_unix_ms(SystemTime::now())
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;

	#[test]
	fn now_unix_ms_is_close_to_inline_computation() {
		let a = now_unix_ms();
		let b = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
		// Sampled microseconds apart — allow generous CI scheduling slop.
		let diff = b.saturating_sub(a);
		assert!(diff < 1_000, "diff = {diff} ms");
	}

	#[test]
	fn system_time_to_unix_ms_floors_pre_epoch_to_zero() {
		let pre = UNIX_EPOCH - Duration::from_secs(1);
		assert_eq!(system_time_to_unix_ms(pre), 0);
	}

	#[test]
	fn system_time_to_unix_ms_round_trips_known_instant() {
		let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
		assert_eq!(system_time_to_unix_ms(t), 1_700_000_000_000);
	}
}
