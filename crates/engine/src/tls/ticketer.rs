//! Daemon-wide TLS session ticketer.
//!
//! Wraps the active crypto backend's `Ticketer::new()` constructor,
//! which returns an `Arc<rustls::TicketRotator>` (RFC 5077 /
//! AES-256-CBC + HMAC-SHA256, 6-hour rotation period, 12-hour ticket
//! lifetime). The rotator self-rolls on the encrypt / decrypt hot
//! path — no background task, no cancellation handling, no `ArcSwap`
//! plumbing.
//!
//! Installed once at boot via [`install_default_ticketer`]; every
//! listener's `ServerConfig.ticketer` reads the same `Arc` via
//! [`default_ticketer`]. Idempotent: a second install is a no-op so
//! daemon main and test harnesses can both invoke without
//! coordination. Mirrors the [`crate::crypto::install_default_provider`]
//! shape.
//!
//! See `spec/crates/engine-tls.md` § _Session ticket rotation_.

use std::sync::{Arc, OnceLock};

use rustls::server::ProducesTickets;

static DEFAULT_TICKETER: OnceLock<Arc<dyn ProducesTickets>> = OnceLock::new();

/// Build a fresh ticketer using the compile-time-selected crypto
/// backend's RFC 5077 constructor. Each call asks the backend for
/// fresh random key material — only the first call's result ends up
/// in the shared `OnceLock`.
fn make_default_ticketer() -> Result<Arc<dyn ProducesTickets>, rustls::Error> {
	#[cfg(feature = "aws-lc-rs")]
	{
		rustls::crypto::aws_lc_rs::Ticketer::new()
	}
	#[cfg(all(feature = "ring", not(feature = "aws-lc-rs")))]
	{
		rustls::crypto::ring::Ticketer::new()
	}
	#[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
	{
		// Unreachable in any legal build — `lib.rs` issues a
		// `compile_error!` when neither backend feature is active —
		// but the function still has to type-check on every cfg
		// branch.
		Err(rustls::Error::General("no crypto backend selected".to_owned()))
	}
}

/// Install the daemon-wide ticketer. Idempotent: a second call after
/// a successful install is a no-op and returns `Ok(())`. Must be
/// called after [`crate::crypto::install_default_provider`] — the
/// backend constructors hit the active provider's RNG.
///
/// # Errors
/// Returns [`rustls::Error`] only when the backend fails to construct
/// the initial ticketer (extremely rare — typically a CSPRNG
/// failure). Daemon main treats this as fatal.
pub fn install_default_ticketer() -> Result<(), rustls::Error> {
	if DEFAULT_TICKETER.get().is_some() {
		return Ok(());
	}
	let t = make_default_ticketer()?;
	let _ = DEFAULT_TICKETER.set(t);
	Ok(())
}

/// Return the installed daemon-wide ticketer, or `None` if no install
/// has happened yet. Test fixtures that don't need session ticketing
/// simply skip [`install_default_ticketer`] and listeners fall back
/// to rustls's default `NeverProducesTickets`.
#[must_use]
pub fn default_ticketer() -> Option<Arc<dyn ProducesTickets>> {
	DEFAULT_TICKETER.get().cloned()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn install_crypto() {
		crate::crypto::install_default_provider();
	}

	// `DEFAULT_TICKETER` is a process-global `OnceLock`. A test that
	// runs before any other test in this binary may observe the
	// pre-install `None`; once any test installs, the state is sticky
	// for the remainder of the process. Each test below tolerates
	// running first or after — assertions only depend on the
	// post-install invariants.

	#[test]
	fn install_then_default_returns_some() {
		install_crypto();
		install_default_ticketer().expect("install ticketer");
		assert!(default_ticketer().is_some());
	}

	#[test]
	fn install_is_idempotent_and_returns_same_arc() {
		install_crypto();
		install_default_ticketer().expect("install ticketer");
		let first = default_ticketer().expect("first read");
		install_default_ticketer().expect("second install is a no-op");
		let second = default_ticketer().expect("second read");
		assert!(Arc::ptr_eq(&first, &second), "OnceLock must hand out the same Arc");
	}

	#[test]
	fn ticketer_round_trips_application_payload() {
		install_crypto();
		install_default_ticketer().expect("install ticketer");
		let t = default_ticketer().expect("ticketer present");
		let plaintext = b"vane-session-payload";
		let cipher = t.encrypt(plaintext).expect("encrypt");
		let recovered = t.decrypt(&cipher).expect("decrypt");
		assert_eq!(recovered.as_slice(), plaintext);
	}
}
