//! RFC 9773 (Auto Renewal Information) client wrapper.
//!
//! ACME directories that support ARI expose a `renewalInfo` URL in
//! their directory document. After every successful issuance the
//! client may query `<renewalInfo>/<base64url(AKI)>.<base64url(serial)>`
//! and receive a CA-suggested
//! `{"suggestedWindow": {"start": "...", "end": "..."}}`.
//!
//! Implementation strategy: `instant-acme` already does the hard
//! parts — directory parsing, JWS signing of the GET-as-POST,
//! `CertificateIdentifier` URL construction, and ARI response JSON
//! deserialisation (see `instant_acme::Account::renewal_info`). This
//! module:
//!
//! 1. Wraps the result in our own [`AriWindow`] type that doesn't
//!    leak `time::OffsetDateTime` into the registry's surface.
//! 2. Translates "directory has no renewalInfo URL" into
//!    [`AriOutcome::Unsupported`] rather than an error class —
//!    callers shouldn't have to string-match on `instant_acme`'s
//!    error to decide whether to retry tomorrow vs. accept the
//!    silent CA.
//!
//! `spec/acme.md` § _ARI (RFC 9773)_ requires the registry to
//! cache the suggested window per cert and trigger renewal when
//! `now ∈ window` — even before `renew_before` would otherwise fire.
//! [`super::scheduler::should_attempt`] picks up the cached
//! [`AriWindow`] from [`super::scheduler::CertState::ari_window`].

use std::time::SystemTime;

use rustls::pki_types::CertificateDer;

use super::registry::RegistryError;

/// Suggested renewal window, parsed from the CA's ARI response.
/// `start..=end` is wall-clock time; the scheduler triggers renewal
/// when `start <= now <= end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AriWindow {
	pub start: SystemTime,
	pub end: SystemTime,
}

impl AriWindow {
	/// `true` when `now` falls inside `[start, end]` (inclusive at
	/// both ends — RFC 9773 §4.2 doesn't specify open/closed and
	/// inclusive is the safer call: a one-second-late tick still
	/// triggers renewal).
	#[must_use]
	pub fn contains(&self, now: SystemTime) -> bool {
		now >= self.start && now <= self.end
	}
}

/// Outcome of a `fetch_window` call. The "no renewalInfo URL" case
/// gets its own variant so callers can stop polling and avoid
/// surfacing a noisy error log on every tick against a CA that
/// simply doesn't support ARI.
#[derive(Debug, Clone)]
pub enum AriOutcome {
	/// CA returned a window. Cache and consult the scheduler.
	Window(AriWindow),
	/// Directory document had no `renewalInfo` field. Permanent for
	/// this account / directory; the registry stops polling.
	Unsupported,
}

/// Translate `instant_acme::SuggestedWindow` (whose dates are
/// `time::OffsetDateTime`) into our `SystemTime`-keyed [`AriWindow`].
fn window_from_instant_acme(
	window: &instant_acme::SuggestedWindow,
) -> Result<AriWindow, RegistryError> {
	let start = offset_to_system(window.start)?;
	let end = offset_to_system(window.end)?;
	Ok(AriWindow { start, end })
}

fn offset_to_system(dt: time::OffsetDateTime) -> Result<SystemTime, RegistryError> {
	let unix = dt.unix_timestamp();
	let unix = u64::try_from(unix).map_err(|_| {
		RegistryError::Acme(format!("ARI suggestedWindow timestamp negative ({unix}); rejecting"))
	})?;
	Ok(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(unix))
}

/// Fetch the ARI window for `cert_der` via `account`'s directory.
/// Returns [`AriOutcome::Unsupported`] when the directory exposes
/// no `renewalInfo` URL or the cert has no AKI extension (the
/// latter would make the URL un-constructable per RFC 9773 §4.1).
///
/// Network / parse failures other than the two "expected silence"
/// cases above surface as [`RegistryError::Acme`] so callers can
/// log and retry on the scheduler's next tick.
///
/// # Errors
/// - [`RegistryError::Acme`] for any HTTP / JSON / x509 parse error
///   that isn't covered by [`AriOutcome::Unsupported`].
pub async fn fetch_window(
	account: &instant_acme::Account,
	cert_der: &CertificateDer<'_>,
) -> Result<AriOutcome, RegistryError> {
	let cert_id = match instant_acme::CertificateIdentifier::try_from(cert_der) {
		Ok(id) => id,
		Err(msg) => {
			// Cert lacks an AKI extension. Some legacy / self-signed
			// chains hit this; from RFC 9773's perspective the cert
			// is simply un-identifiable for ARI, so we return
			// Unsupported (no error noise on every tick).
			tracing::debug!(
				target: "vane::acme::ari",
				error = %msg,
				"cert has no Authority Key Identifier; ARI not applicable",
			);
			return Ok(AriOutcome::Unsupported);
		}
	};

	match account.renewal_info(&cert_id).await {
		Ok((info, _retry_after)) => {
			let window = window_from_instant_acme(&info.suggested_window)?;
			tracing::debug!(
				target: "vane::acme::ari",
				start = ?window.start,
				end = ?window.end,
				"ARI window resolved",
			);
			Ok(AriOutcome::Window(window))
		}
		Err(e) => {
			// `instant_acme::Account::renewal_info` returns an error
			// when the directory has no `renewalInfo` URL. We can't
			// distinguish that from a transient HTTP error without
			// inspecting the message, so we hint by string-match —
			// imperfect, but the alternative is unconditionally
			// classifying CA silence as an error.
			let msg = e.to_string();
			if msg.contains("renewalInfo") || msg.contains("does not support") {
				return Ok(AriOutcome::Unsupported);
			}
			Err(RegistryError::Acme(format!("ARI fetch: {msg}")))
		}
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;

	#[test]
	fn ari_window_contains_at_endpoints() {
		let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
		let end = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
		let window = AriWindow { start, end };
		assert!(window.contains(start));
		assert!(window.contains(end));
		assert!(window.contains(start + Duration::from_secs(500)));
		assert!(!window.contains(start - Duration::from_secs(1)));
		assert!(!window.contains(end + Duration::from_secs(1)));
	}

	#[test]
	fn offset_to_system_round_trips_unix_seconds() {
		let secs: i64 = 1_700_000_000;
		let dt = time::OffsetDateTime::from_unix_timestamp(secs).unwrap();
		let st = offset_to_system(dt).unwrap();
		let unix = st.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
		assert_eq!(unix, secs.cast_unsigned());
	}

	#[test]
	fn offset_to_system_rejects_negative_epoch() {
		// Pre-1970 timestamps should never surface from a CA, but if
		// one did we surface as RegistryError rather than a u64 wrap.
		let dt = time::OffsetDateTime::from_unix_timestamp(-1).unwrap();
		let err = offset_to_system(dt).unwrap_err();
		match err {
			RegistryError::Acme(msg) => assert!(msg.contains("negative"), "{msg}"),
			other => panic!("expected Acme, got {other:?}"),
		}
	}

	#[test]
	fn window_from_instant_acme_translates_dates() {
		let start_secs: i64 = 1_700_000_000;
		let end_secs: i64 = 1_700_086_400;
		let window = instant_acme::SuggestedWindow {
			start: time::OffsetDateTime::from_unix_timestamp(start_secs).unwrap(),
			end: time::OffsetDateTime::from_unix_timestamp(end_secs).unwrap(),
		};
		let translated = window_from_instant_acme(&window).unwrap();
		assert_eq!(
			translated.start.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(),
			start_secs.cast_unsigned(),
		);
		assert_eq!(
			translated.end.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(),
			end_secs.cast_unsigned(),
		);
	}
}
