//! Renewal scheduler types + decision logic per `spec/crates/engine-acme.md`
//! § _Renewal triggers_ and § _Rate-limit and failure handling_.
//!
//! Three pieces:
//!
//! - [`CertStatus`] / [`CertState`]: per-SNI runtime state (status,
//!   last error, backoff timestamps, current cert) — distinct from
//!   the persistable [`super::store::StoredCert`] and updated by
//!   [`super::ManagedCertRegistry::record_success`] /
//!   `record_failure` after every issuance attempt.
//! - [`RenewalJob`]: how to retry a given SNI — directory URL,
//!   contact list, challenge kind, DNS provider handle when the
//!   challenge is dns-01. Registered once at boot so the scheduler
//!   tick (and the `force_renew` mgmt verb) can dispatch without
//!   re-walking the listener spec.
//! - [`next_backoff`] / [`collect_renewal_plans`]: pure decision
//!   logic. Tested directly with synthesised state inputs so the
//!   scheduler tick is a thin shell around these functions.
//!
//! Backoff per spec § _Rate-limit and failure handling_: base 30
//! minutes, factor 2, cap 24 hours; resets to base on first success.
//! Both rate-limited and other-failure classes use the same
//! schedule; rate-limited responses additionally honour the CA's
//! `Retry-After` header when it exceeds the local backoff.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use vane_core::rule::ChallengeKind;

use super::ari::AriWindow;
use super::dns::DnsProvider;
use super::store::StoredCert;

/// Per-SNI lifecycle state. Surfaces through the `get_certs` mgmt
/// verb so operators can distinguish "haven't issued yet" from
/// "tried and got rate-limited" without scraping logs.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum CertStatus {
	/// A cert is cached and not under active renewal. The hot-path
	/// state for steady-state managed certs.
	Valid,
	/// Renewal in flight (timer / ARI / `force_renew` triggered). No
	/// new attempt is dispatched until this clears.
	Renewing,
	/// Last attempt failed (network / DNS / validation timeout).
	/// `next_attempt_at` is populated; the scheduler tick respects it.
	Failed,
	/// Last attempt hit `urn:ietf:params:acme:error:rateLimited`.
	/// `next_attempt_at` honours the CA's `Retry-After` when it
	/// exceeded the local backoff schedule.
	Limited,
}

/// Per-SNI runtime state. Held inside
/// [`super::ManagedCertRegistry::certs`]; cloned cheaply (every
/// stored field is `Arc` / scalar) when handed to mgmt-verb
/// consumers.
#[derive(Debug, Clone)]
pub struct CertState {
	/// The currently cached cert, if any. `None` between "SNI
	/// declared" and "first issuance landed".
	pub stored: Option<Arc<StoredCert>>,
	/// Lifecycle position. Drives both the renewal scheduler's
	/// "should I attempt now?" decision and the mgmt verb's surface.
	pub status: CertStatus,
	/// Wall-clock of the last attempt (success OR failure). `None`
	/// before any attempt has run.
	pub last_attempt_at: Option<SystemTime>,
	/// Last attempt's error, if any. Cleared on first success.
	pub last_error: Option<String>,
	/// Earliest wall-clock the scheduler may dispatch the next
	/// attempt. `None` for `Valid` / fresh state; populated for
	/// `Failed` / `Limited`.
	pub next_attempt_at: Option<SystemTime>,
	/// How many failures have stacked since the last success. Used
	/// by [`next_backoff`] to compute the next gap. Reset to 0 on
	/// success.
	pub consecutive_failures: u32,
	/// Most recent CA-suggested ARI window per RFC 9773. `None`
	/// when the directory hasn't returned a window yet, when the
	/// directory doesn't support ARI, or when the cert lacks an
	/// AKI extension. The renewal scheduler triggers on
	/// `now ∈ window` membership in addition to the `renew_before`
	/// threshold.
	pub ari_window: Option<AriWindow>,
}

impl CertState {
	/// Initial state for an SNI we know about but have never
	/// attempted. Used by `declare_managed` and by hydrate when a
	/// stored cert exists.
	#[must_use]
	pub fn fresh(stored: Option<Arc<StoredCert>>) -> Self {
		Self {
			stored,
			status: CertStatus::Valid,
			last_attempt_at: None,
			last_error: None,
			next_attempt_at: None,
			consecutive_failures: 0,
			ari_window: None,
		}
	}
}

/// How to retry issuance for one SNI. Registered at daemon boot
/// (per `acme_boot.rs::collect_issuance_plans`), captured here so
/// the scheduler can dispatch without reparsing listener specs and
/// the daemon can drop its boot-only `IssuancePlan` list.
#[derive(Clone)]
pub struct RenewalJob {
	pub directory_url: String,
	pub contact: Vec<String>,
	pub challenge: ChallengeKind,
	/// `Some` only when `challenge == Dns01`. Pre-built at boot so
	/// the scheduler doesn't have to hold a JSON config + know how
	/// to dispatch on `kind` discriminators.
	pub dns: Option<Arc<dyn DnsProvider>>,
	/// `now + renew_before >= not_after` triggers renewal. Per-cert
	/// per spec § _Configuration schema_; CLI/TUI default `30d`.
	pub renew_before: Duration,
	/// Optional CA root for the `instant-acme` HTTP client — only
	/// populated by Pebble integration tests (production CAs use
	/// public roots).
	pub extra_root_ca_pem: Option<PathBuf>,
}

impl std::fmt::Debug for RenewalJob {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("RenewalJob")
			.field("directory_url", &self.directory_url)
			.field("contact", &self.contact)
			.field("challenge", &self.challenge)
			.field("renew_before", &self.renew_before)
			.field("has_dns", &self.dns.is_some())
			.field("extra_root_ca_pem", &self.extra_root_ca_pem)
			.finish()
	}
}

/// One scheduler tick's "this SNI needs an attempt" verdict, with
/// the job payload pre-resolved for the spawn point.
#[derive(Debug, Clone)]
pub struct RenewalPlan {
	pub sni: String,
	pub job: RenewalJob,
}

/// Periodic scheduler cadence per `spec/crates/engine-acme.md`
/// § _Renewal triggers § Periodic timer_. Matches `spec/crates/engine-tls.md`'s
/// `refresh()` cadence so cert delivery and renewal share one
/// heartbeat.
pub const TICK_INTERVAL: Duration = Duration::from_mins(5);

/// Refresh the cached OCSP staple when the responder's
/// `nextUpdate` falls inside this window. Per `spec/crates/engine-tls.md` § _OCSP
/// stapling_, OCSP responses typically validate for 4–7 days and
/// the spec recommends "refresh daily"; 24 h gives a comfortable
/// margin (one tick at default cadence overshooting still leaves
/// hours of headroom before the staple expires).
pub const OCSP_REFRESH_BEFORE: Duration = Duration::from_hours(24);

/// Backoff base — first failure waits this long before retry. Per
/// `spec/crates/engine-acme.md` § _Rate-limit and failure handling_.
pub const BACKOFF_BASE: Duration = Duration::from_mins(30);

/// Backoff cap — never wait longer than this between attempts. Per
/// the same spec section.
pub const BACKOFF_CAP: Duration = Duration::from_hours(24);

/// Compute the next backoff gap given how many consecutive failures
/// have stacked since the last success. The first failure
/// (`consecutive_failures == 0` at the time of the failure, becomes
/// 1 after [`record_failure`]) waits [`BACKOFF_BASE`]; each
/// subsequent failure doubles, saturating at [`BACKOFF_CAP`].
///
/// Saturating-shift on the exponent: at 20 failures the bare doubling
/// would already overflow `u64` seconds, so we clamp the shift count
/// to a value whose product cannot exceed `u64::MAX` regardless of
/// `BACKOFF_BASE` and let [`BACKOFF_CAP`] take over the result.
#[must_use]
pub fn next_backoff(consecutive_failures: u32) -> Duration {
	if consecutive_failures == 0 {
		return BACKOFF_BASE;
	}
	let exp = consecutive_failures.saturating_sub(1).min(20);
	let multiplier: u64 = 1u64 << exp;
	let secs = BACKOFF_BASE.as_secs().saturating_mul(multiplier);
	let candidate = Duration::from_secs(secs);
	if candidate > BACKOFF_CAP { BACKOFF_CAP } else { candidate }
}

/// Record a successful attempt onto `state`. Resets the failure
/// counter, clears any error, sets status to [`CertStatus::Valid`]
/// and stamps the new cert.
pub fn record_success(state: &mut CertState, stored: Arc<StoredCert>, now: SystemTime) {
	state.stored = Some(stored);
	state.status = CertStatus::Valid;
	state.last_attempt_at = Some(now);
	state.last_error = None;
	state.next_attempt_at = None;
	state.consecutive_failures = 0;
}

/// Record a failed attempt onto `state`. `rate_limited` selects
/// between [`CertStatus::Limited`] and [`CertStatus::Failed`];
/// `retry_after` is the CA's suggestion when the response carried a
/// `Retry-After` header — we honour the larger of (server suggestion,
/// local backoff) to avoid being more aggressive than the CA wants.
pub fn record_failure(
	state: &mut CertState,
	error: String,
	rate_limited: bool,
	retry_after: Option<Duration>,
	now: SystemTime,
) {
	state.consecutive_failures = state.consecutive_failures.saturating_add(1);
	state.last_attempt_at = Some(now);
	state.last_error = Some(error);
	let local_backoff = next_backoff(state.consecutive_failures);
	let effective = match retry_after {
		Some(server) if server > local_backoff => server,
		_ => local_backoff,
	};
	state.next_attempt_at = Some(now + effective);
	state.status = if rate_limited { CertStatus::Limited } else { CertStatus::Failed };
}

/// Mark `state` as in-flight: the scheduler is about to dispatch a
/// renewal task. Idempotent — a second caller observing
/// [`CertStatus::Renewing`] should bail out.
pub fn mark_renewing(state: &mut CertState, now: SystemTime) {
	state.status = CertStatus::Renewing;
	state.last_attempt_at = Some(now);
}

/// Pure-decision: should the scheduler dispatch an OCSP refresh for
/// `state` at `now`? Two trigger conditions per
/// `spec/crates/engine-acme.md` § _OCSP stapling_:
///
/// - `state.stored.is_some()` AND no staple has been cached yet
///   (`ocsp_response.is_none()`) AND a responder URL is known
///   (`ocsp_aia_url.is_some()`): a previous fetch failed; retry on
///   this tick.
/// - `ocsp_response.is_some()` AND
///   `now + OCSP_REFRESH_BEFORE >= ocsp_next_update`: the staple
///   is approaching its `nextUpdate`; refresh proactively.
///
/// Returns `false` when no cert is cached (nothing to staple) or
/// when no AIA URL is known (the cert legitimately has no
/// responder; nothing to refresh).
#[must_use]
pub fn should_refresh_ocsp(state: &CertState, now: SystemTime) -> bool {
	let Some(stored) = &state.stored else { return false };
	if stored.ocsp_aia_url.is_none() {
		return false;
	}
	match (&stored.ocsp_response, stored.ocsp_next_update) {
		(None, _) => true,
		(Some(_), None) => false,
		(Some(_), Some(next_update)) => match next_update.checked_sub(OCSP_REFRESH_BEFORE) {
			Some(deadline) => now >= deadline,
			None => true,
		},
	}
}

/// Decide whether `state` warrants a renewal attempt at `now` for a
/// cert that was issued under `job` (specifically: `job.renew_before`
/// + the cert's `not_after`, plus any cached ARI window).
///
/// Triggers per spec § _Renewal triggers_:
///
/// - status `Valid` AND `now + renew_before >= not_after` (timer);
///   when no cert is cached yet (first-time issuance never ran), the
///   timer also fires so the scheduler picks up newly-declared SNIs.
/// - status `Valid` AND `now ∈ ari_window` (RFC 9773): renew even
///   before `renew_before` would otherwise fire; this lets CAs
///   spread renewal load and signal forced rotation.
/// - status `Failed` / `Limited` AND `now >= next_attempt_at` (backoff
///   elapsed).
/// - status `Renewing` is always skipped (already in flight).
#[must_use]
pub fn should_attempt(state: &CertState, job: &RenewalJob, now: SystemTime) -> bool {
	match state.status {
		CertStatus::Renewing => false,
		CertStatus::Valid => {
			if let Some(window) = &state.ari_window
				&& window.contains(now)
			{
				return true;
			}
			match &state.stored {
				None => true,
				Some(stored) => match stored.not_after.checked_sub(job.renew_before) {
					Some(deadline) => now >= deadline,
					None => true,
				},
			}
		}
		CertStatus::Failed | CertStatus::Limited => {
			state.next_attempt_at.is_none_or(|next| now >= next)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn dummy_job() -> RenewalJob {
		RenewalJob {
			directory_url: "https://acme.invalid/dir".into(),
			contact: vec!["mailto:ops@example.com".into()],
			challenge: ChallengeKind::Http01,
			dns: None,
			renew_before: Duration::from_hours(720),
			extra_root_ca_pem: None,
		}
	}

	fn dummy_stored(not_after: SystemTime) -> Arc<StoredCert> {
		Arc::new(StoredCert {
			leaf_pem: "leaf".into(),
			chain_pem: String::new(),
			key_pem: "key".into(),
			not_after,
			ari_replacement_id: None,
			last_renew_at: SystemTime::UNIX_EPOCH,
			ocsp_response: None,
			ocsp_next_update: None,
			ocsp_aia_url: None,
		})
	}

	#[test]
	fn next_backoff_starts_at_base_for_first_failure() {
		assert_eq!(next_backoff(0), BACKOFF_BASE);
		assert_eq!(next_backoff(1), BACKOFF_BASE);
	}

	#[test]
	fn next_backoff_doubles_each_failure() {
		assert_eq!(next_backoff(2), BACKOFF_BASE * 2);
		assert_eq!(next_backoff(3), BACKOFF_BASE * 4);
		assert_eq!(next_backoff(4), BACKOFF_BASE * 8);
	}

	#[test]
	fn next_backoff_caps_at_24h() {
		// 30 min * 2^6 = 32 hours > 24h cap.
		assert_eq!(next_backoff(7), BACKOFF_CAP);
		assert_eq!(next_backoff(20), BACKOFF_CAP);
		assert_eq!(next_backoff(u32::MAX), BACKOFF_CAP);
	}

	#[test]
	fn record_success_resets_failure_counter() {
		let mut state = CertState::fresh(None);
		state.consecutive_failures = 4;
		state.status = CertStatus::Failed;
		state.last_error = Some("boom".into());
		state.next_attempt_at = Some(SystemTime::UNIX_EPOCH);
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
		let stored = dummy_stored(now + Duration::from_hours(24));
		record_success(&mut state, Arc::clone(&stored), now);
		assert_eq!(state.status, CertStatus::Valid);
		assert_eq!(state.consecutive_failures, 0);
		assert!(state.last_error.is_none());
		assert!(state.next_attempt_at.is_none());
		assert_eq!(state.last_attempt_at, Some(now));
		assert!(Arc::ptr_eq(state.stored.as_ref().unwrap(), &stored));
	}

	#[test]
	fn record_failure_uses_retry_after_when_larger_than_backoff() {
		let mut state = CertState::fresh(None);
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
		// Server says retry in 4h; local backoff for first failure
		// is 30 min, so the server suggestion wins.
		record_failure(&mut state, "rate".into(), true, Some(Duration::from_hours(4)), now);
		assert_eq!(state.status, CertStatus::Limited);
		assert_eq!(state.consecutive_failures, 1);
		assert_eq!(state.next_attempt_at, Some(now + Duration::from_hours(4)));
	}

	#[test]
	fn record_failure_uses_local_backoff_when_retry_after_smaller() {
		let mut state = CertState::fresh(None);
		state.consecutive_failures = 4; // already pretty backed off
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
		// 4 prior failures + this one = 5; backoff = 30min * 2^4 = 8h.
		// Server says retry in 1h; local wins.
		record_failure(&mut state, "boom".into(), false, Some(Duration::from_hours(1)), now);
		assert_eq!(state.status, CertStatus::Failed);
		assert_eq!(state.consecutive_failures, 5);
		let expected_gap = next_backoff(5);
		assert_eq!(state.next_attempt_at, Some(now + expected_gap));
	}

	#[test]
	fn record_failure_classifies_rate_limited_vs_other() {
		let mut state = CertState::fresh(None);
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
		record_failure(&mut state, "x".into(), false, None, now);
		assert_eq!(state.status, CertStatus::Failed);
		// Reset and try the rate-limited branch.
		let mut state2 = CertState::fresh(None);
		record_failure(&mut state2, "x".into(), true, None, now);
		assert_eq!(state2.status, CertStatus::Limited);
	}

	#[test]
	fn should_attempt_skips_renewing() {
		let mut state = CertState::fresh(None);
		state.status = CertStatus::Renewing;
		assert!(!should_attempt(&state, &dummy_job(), SystemTime::UNIX_EPOCH));
	}

	#[test]
	fn should_attempt_fires_when_no_cert_cached() {
		let state = CertState::fresh(None);
		assert!(should_attempt(&state, &dummy_job(), SystemTime::UNIX_EPOCH));
	}

	#[test]
	fn should_attempt_respects_renew_before_threshold() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
		let renew_before = Duration::from_hours(1);
		let mut job = dummy_job();
		job.renew_before = renew_before;
		// not_after = now + 90 min: now + 60min < not_after → don't renew.
		let stored = dummy_stored(now + Duration::from_mins(90));
		let state = CertState::fresh(Some(stored));
		assert!(!should_attempt(&state, &job, now));
		// not_after = now + 30 min: now + 60min > not_after → renew.
		let stored2 = dummy_stored(now + Duration::from_mins(30));
		let state2 = CertState::fresh(Some(stored2));
		assert!(should_attempt(&state2, &job, now));
	}

	#[test]
	fn should_attempt_fires_on_ari_window_membership() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
		let mut job = dummy_job();
		// Set renew_before tight so the timer alone wouldn't fire.
		job.renew_before = Duration::from_mins(1);
		// Cert valid for another year — far past `renew_before`.
		let stored = dummy_stored(now + Duration::from_hours(8760));
		let mut state = CertState::fresh(Some(stored));
		// Without ARI window, no renewal yet.
		assert!(!should_attempt(&state, &job, now));
		// CA-suggested window covers `now` — renew despite the
		// timer being satisfied.
		state.ari_window =
			Some(AriWindow { start: now - Duration::from_mins(1), end: now + Duration::from_mins(1) });
		assert!(should_attempt(&state, &job, now));
	}

	#[test]
	fn should_attempt_skips_when_ari_window_in_future() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
		let mut job = dummy_job();
		job.renew_before = Duration::from_mins(1);
		let stored = dummy_stored(now + Duration::from_hours(8760));
		let mut state = CertState::fresh(Some(stored));
		state.ari_window =
			Some(AriWindow { start: now + Duration::from_hours(2), end: now + Duration::from_hours(4) });
		assert!(!should_attempt(&state, &job, now), "future window doesn't fire yet");
	}

	#[test]
	fn should_attempt_respects_next_attempt_for_failed_state() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
		let mut state = CertState::fresh(None);
		state.status = CertStatus::Failed;
		state.next_attempt_at = Some(now + Duration::from_mins(1));
		assert!(!should_attempt(&state, &dummy_job(), now));
		state.next_attempt_at = Some(now - Duration::from_secs(1));
		assert!(should_attempt(&state, &dummy_job(), now));
	}

	#[test]
	fn should_refresh_ocsp_skips_when_no_cert() {
		let state = CertState::fresh(None);
		assert!(!should_refresh_ocsp(&state, SystemTime::UNIX_EPOCH));
	}

	#[test]
	fn should_refresh_ocsp_skips_when_no_aia_url() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
		let stored = dummy_stored(now + Duration::from_hours(24 * 30));
		let state = CertState::fresh(Some(stored));
		assert!(!should_refresh_ocsp(&state, now));
	}

	#[test]
	fn should_refresh_ocsp_fires_when_aia_known_but_no_staple() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
		let mut stored_inner = (*dummy_stored(now + Duration::from_hours(24 * 30))).clone();
		stored_inner.ocsp_aia_url = Some("http://ocsp.example.test/".into());
		let state = CertState::fresh(Some(Arc::new(stored_inner)));
		assert!(should_refresh_ocsp(&state, now), "missing staple → fetch");
	}

	#[test]
	fn should_refresh_ocsp_fires_when_within_refresh_window() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
		let mut stored_inner = (*dummy_stored(now + Duration::from_hours(24 * 30))).clone();
		stored_inner.ocsp_aia_url = Some("http://ocsp.example.test/".into());
		stored_inner.ocsp_response = Some(b"DER".to_vec());
		// next_update inside the 24h window from now → refresh.
		stored_inner.ocsp_next_update = Some(now + Duration::from_hours(12));
		let state = CertState::fresh(Some(Arc::new(stored_inner)));
		assert!(should_refresh_ocsp(&state, now));
	}

	#[test]
	fn should_refresh_ocsp_skips_when_staple_still_fresh() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
		let mut stored_inner = (*dummy_stored(now + Duration::from_hours(24 * 30))).clone();
		stored_inner.ocsp_aia_url = Some("http://ocsp.example.test/".into());
		stored_inner.ocsp_response = Some(b"DER".to_vec());
		// next_update beyond the 24h window → no refresh yet.
		stored_inner.ocsp_next_update = Some(now + Duration::from_hours(48));
		let state = CertState::fresh(Some(Arc::new(stored_inner)));
		assert!(!should_refresh_ocsp(&state, now));
	}

	#[test]
	fn mark_renewing_blocks_subsequent_attempt_decision() {
		let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
		let mut state = CertState::fresh(None);
		mark_renewing(&mut state, now);
		assert_eq!(state.status, CertStatus::Renewing);
		assert_eq!(state.last_attempt_at, Some(now));
		assert!(!should_attempt(&state, &dummy_job(), now));
	}
}
