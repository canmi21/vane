//! Persistent disk-backed TLS session ticketer.
//!
//! `rustls::crypto::*::Ticketer::new()` generates a fresh random key
//! every process, so a daemon restart invalidates every issued
//! session ticket and forces every connecting client through a full
//! TLS 1.2 handshake (no resumption) or a 1-RTT TLS 1.3 handshake
//! (no early data). For long-lived control planes that operators
//! restart on every config push this is a measurable handshake-rate
//! cliff — and the key is unrecoverable so future-secrecy is
//! preserved for ticket-based resumption either way.
//!
//! This module persists the key on disk so it survives a restart.
//! The ticket itself is AES-256-GCM encrypted with the on-disk key
//! using a fresh random nonce per ticket; the ciphertext layout is
//! `[version_byte_0x01 | nonce_12B | ciphertext+tag]`. The version
//! byte gates future migrations — a daemon reading a ticket whose
//! first byte isn't `0x01` returns `None` from `decrypt`, which
//! rustls maps to "ticket rejected, fall back to full handshake".

#![allow(unsafe_code, reason = "libc::umask FFI; tight scope around the file create.")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aws_lc_rs::aead::{AES_256_GCM, LessSafeKey, Nonce, NonceSequence, UnboundKey};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use rustls::server::ProducesTickets;
use zeroize::Zeroizing;

use crate::DEFAULT_TICKETER;

/// AES-256-GCM key length in bytes.
const KEY_LEN: usize = 32;
/// AES-GCM nonce length in bytes (per `aws_lc_rs::aead::NONCE_LEN`).
const NONCE_LEN: usize = 12;
/// Version byte at the head of every encrypted ticket. Lets a future
/// rotation scheme migrate ticket formats without invalidating the
/// in-flight ticket cache in one step.
const TICKET_VERSION: u8 = 0x01;
/// rustls's default ticket lifetime — 12 hours. Matches the auto-
/// rotating `Ticketer::new()` so operator-visible session-resumption
/// windows are identical between the persistent and non-persistent
/// installs.
const TICKET_LIFETIME_SECS: u32 = 12 * 60 * 60;

/// Install a disk-backed ticketer keyed on `path`. The path receives
/// `0o600` perms (private file, mode-aware create on unix) and the
/// key bytes are zeroized in memory once the ticketer Arc drops.
///
/// Boot behaviour:
/// - Path exists, file size matches `KEY_LEN`: load and reuse. Issued
///   tickets from the prior daemon process keep resolving.
/// - Path missing or has the wrong size: generate a fresh key, write
///   atomically (tempfile + persist), continue.
/// - Path readable but the bytes are corrupted / wrong length: log a
///   warn, overwrite with a fresh key (the corrupted bytes can never
///   decrypt anyway).
///
/// Idempotent: a second install after a successful first is a no-op
/// (`OnceLock::set` returns `Err` quietly). Reinstalling under a
/// different `path` after a successful first install does **not**
/// swap the ticketer — operators wishing to rotate via path change
/// must restart the daemon.
///
/// # Errors
/// Returns [`rustls::Error::General`] when file I/O or key creation
/// fails. The daemon's boot path should treat the error as fatal;
/// the fallback is to skip ticketing entirely (resumption disabled).
pub fn install_persistent_ticketer(path: &Path) -> Result<(), rustls::Error> {
	if DEFAULT_TICKETER.get().is_some() {
		return Ok(());
	}
	let key = load_or_generate_key(path).map_err(|e| {
		rustls::Error::General(format!("persistent ticketer key at {}: {e}", path.display()))
	})?;
	let ticketer = Arc::new(PersistentTicketer::new(key.as_slice())?) as Arc<dyn ProducesTickets>;
	let _ = DEFAULT_TICKETER.set(ticketer);
	Ok(())
}

/// Read the key file when present and well-formed; otherwise
/// generate a fresh 32-byte key and persist it atomically.
///
/// The returned key is wrapped in `Zeroizing` so the wipe-on-drop
/// contract holds for any transient copy on the install path.
fn load_or_generate_key(path: &Path) -> std::io::Result<Zeroizing<[u8; KEY_LEN]>> {
	match std::fs::read(path) {
		Ok(bytes) if bytes.len() == KEY_LEN => {
			let mut out = Zeroizing::new([0u8; KEY_LEN]);
			out.copy_from_slice(&bytes);
			tracing::info!(path = %path.display(), "ticketer key loaded from disk");
			Ok(out)
		}
		Ok(bytes) => {
			tracing::warn!(
				path = %path.display(),
				size = bytes.len(),
				"ticketer key file has unexpected size; regenerating",
			);
			generate_and_persist_key(path)
		}
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => generate_and_persist_key(path),
		Err(e) => Err(e),
	}
}

fn generate_and_persist_key(path: &Path) -> std::io::Result<Zeroizing<[u8; KEY_LEN]>> {
	let mut key = Zeroizing::new([0u8; KEY_LEN]);
	SystemRandom::new()
		.fill(key.as_mut_slice())
		.map_err(|_| std::io::Error::other("aws-lc-rs SystemRandom::fill failed"))?;
	persist_key(path, key.as_slice())?;
	tracing::info!(path = %path.display(), "ticketer key generated and persisted");
	Ok(key)
}

/// Atomic write: NamedTempFile + sync_data + persist. The parent
/// directory must exist; callers (the daemon boot path) create
/// `<acme_root>` before calling this.
fn persist_key(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
	use std::io::Write as _;
	let parent = path.parent().ok_or_else(|| {
		std::io::Error::new(std::io::ErrorKind::InvalidInput, "ticketer key path has no parent")
	})?;
	if !parent.exists() {
		std::fs::create_dir_all(parent)?;
	}
	// Tighten umask around the temp file's create so the ticket key
	// is never world-readable in the persist window. Restore on
	// drop. The same RAII trick is used by `ndjson-rpc`'s socket
	// bind path; centralising it would pull in a new dep, and this
	// module is the only other caller.
	let _restore = TighterUmask::new(0o177);
	let tmp = tempfile::NamedTempFile::new_in(parent)?;
	let path_buf: PathBuf = tmp.path().to_path_buf();
	// SAFETY: chmod via std::os::unix::fs::PermissionsExt is safe.
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;
		std::fs::set_permissions(&path_buf, std::fs::Permissions::from_mode(0o600))?;
	}
	let mut handle = tmp.as_file();
	handle.write_all(bytes)?;
	handle.sync_data()?;
	tmp.persist(path).map_err(|e| e.error)?;
	Ok(())
}

/// RAII helper that tightens the process umask for the lifetime of
/// the guard. Identical in spirit to `ndjson_rpc::UmaskRestore`;
/// dup'd here to keep this crate self-contained.
struct TighterUmask {
	#[cfg(unix)]
	previous: libc::mode_t,
}

impl TighterUmask {
	#[cfg(unix)]
	fn new(mask: libc::mode_t) -> Self {
		// SAFETY: `umask` is a thread-safe POSIX call with no
		// preconditions.
		let previous = unsafe { libc::umask(mask) };
		Self { previous }
	}

	#[cfg(not(unix))]
	#[allow(clippy::unused_self)]
	fn new(_mask: u32) -> Self {
		Self {}
	}
}

#[cfg(unix)]
impl Drop for TighterUmask {
	fn drop(&mut self) {
		// SAFETY: as above.
		unsafe {
			libc::umask(self.previous);
		}
	}
}

/// AES-256-GCM ticketer. Holds the unbound key behind a `LessSafeKey`
/// (the aws-lc-rs naming convention; "less safe" here only means the
/// caller owns nonce management — which we do correctly via fresh
/// random per call).
struct PersistentTicketer {
	key: LessSafeKey,
}

impl std::fmt::Debug for PersistentTicketer {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("PersistentTicketer").finish_non_exhaustive()
	}
}

impl PersistentTicketer {
	fn new(key: &[u8]) -> Result<Self, rustls::Error> {
		let unbound = UnboundKey::new(&AES_256_GCM, key).map_err(|_| {
			rustls::Error::General("aws-lc-rs UnboundKey rejected 32-byte AES-256-GCM key".to_owned())
		})?;
		Ok(Self { key: LessSafeKey::new(unbound) })
	}
}

impl ProducesTickets for PersistentTicketer {
	fn enabled(&self) -> bool {
		true
	}

	fn lifetime(&self) -> u32 {
		TICKET_LIFETIME_SECS
	}

	fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>> {
		let mut nonce_bytes = [0u8; NONCE_LEN];
		SystemRandom::new().fill(&mut nonce_bytes).ok()?;
		let nonce = Nonce::assume_unique_for_key(nonce_bytes);
		// Buffer layout: [version | nonce | ciphertext+tag]. Build it
		// up by appending the tag in-place after `seal_in_place_append_tag`.
		let mut buf = Vec::with_capacity(1 + NONCE_LEN + plain.len() + AES_256_GCM.tag_len());
		buf.push(TICKET_VERSION);
		buf.extend_from_slice(&nonce_bytes);
		let mut ciphertext = plain.to_vec();
		self
			.key
			.seal_in_place_append_tag(nonce, aws_lc_rs::aead::Aad::empty(), &mut ciphertext)
			.ok()?;
		buf.extend_from_slice(&ciphertext);
		Some(buf)
	}

	fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
		// Minimum length: version + nonce + tag (no payload bytes).
		if cipher.len() < 1 + NONCE_LEN + AES_256_GCM.tag_len() {
			return None;
		}
		if cipher[0] != TICKET_VERSION {
			return None;
		}
		let mut nonce_bytes = [0u8; NONCE_LEN];
		nonce_bytes.copy_from_slice(&cipher[1..=NONCE_LEN]);
		let nonce = Nonce::assume_unique_for_key(nonce_bytes);
		let mut buf = cipher[1 + NONCE_LEN..].to_vec();
		let plain = self.key.open_in_place(nonce, aws_lc_rs::aead::Aad::empty(), &mut buf).ok()?;
		Some(plain.to_vec())
	}
}

// `Nonce`'s `NonceSequence` impl exists for callers that want
// auto-incrementing nonces; ours generates fresh per call so we
// don't need the trait. Pull it in to silence the "unused import"
// lint when the upstream API changes shape.
#[allow(dead_code)]
const _: fn() = || {
	fn assert_nonce_sequence_supported<T: NonceSequence>() {}
	// Not actually called; this exists so we depend on the trait
	// being present without instantiating one.
};

#[cfg(test)]
mod tests {
	use super::*;

	fn install_crypto_once() {
		let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
	}

	#[test]
	fn round_trip_through_persistent_ticketer() {
		install_crypto_once();
		let key = [9u8; KEY_LEN];
		let t = PersistentTicketer::new(&key).expect("build");
		let plain = b"vane-session-payload";
		let cipher = t.encrypt(plain).expect("encrypt");
		assert_eq!(cipher[0], TICKET_VERSION, "version byte stamped");
		let back = t.decrypt(&cipher).expect("decrypt");
		assert_eq!(back, plain);
	}

	#[test]
	fn decrypt_rejects_unknown_version_byte() {
		install_crypto_once();
		let key = [9u8; KEY_LEN];
		let t = PersistentTicketer::new(&key).expect("build");
		let plain = b"x";
		let mut cipher = t.encrypt(plain).expect("encrypt");
		cipher[0] = 0xFF;
		assert!(t.decrypt(&cipher).is_none(), "wrong-version ticket must be rejected");
	}

	#[test]
	fn decrypt_rejects_truncated_input() {
		install_crypto_once();
		let key = [9u8; KEY_LEN];
		let t = PersistentTicketer::new(&key).expect("build");
		assert!(t.decrypt(b"").is_none());
		assert!(t.decrypt(&[TICKET_VERSION]).is_none());
		assert!(t.decrypt(&[TICKET_VERSION; 12]).is_none());
	}

	#[test]
	fn persist_then_load_yields_same_key_material() {
		install_crypto_once();
		let dir = tempfile::tempdir().expect("tempdir");
		let path = dir.path().join("ticketer.bin");
		let first = load_or_generate_key(&path).expect("first generate");
		let second = load_or_generate_key(&path).expect("second load");
		assert_eq!(first.as_slice(), second.as_slice(), "load returns the persisted key verbatim");
	}

	#[test]
	fn install_persistent_then_default_returns_some() {
		install_crypto_once();
		let dir = tempfile::tempdir().expect("tempdir");
		install_persistent_ticketer(&dir.path().join("ticketer.bin")).expect("install");
		assert!(crate::default_ticketer().is_some());
	}
}
