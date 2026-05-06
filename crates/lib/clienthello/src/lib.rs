//! Extract the TLS Server Name Indication (SNI) from a QUIC client's
//! Initial datagrams without performing a full QUIC handshake.
//!
//! QUIC Initial packets carry the TLS `ClientHello` in CRYPTO frames,
//! AEAD-encrypted with keys derived from the client's chosen
//! Destination Connection ID (RFC 9001 §5.2). Any party with the DCID
//! can decrypt — the secret material the server eventually negotiates
//! during the handshake is not used at the Initial layer. This crate
//! exposes that primitive: feed it raw datagrams as they arrive,
//! get back the SNI when enough of the `ClientHello` has been seen.
//!
//! Use cases include SNI-aware UDP load balancers, observability
//! probes, and any system that needs to route QUIC connections by
//! server name without terminating them.
//!
//! # Versions
//!
//! Currently supports **QUIC v1** (transport version `0x00000001`,
//! RFC 9000). v2 (RFC 9369) salt + TLS 1.3 cipher suite are mechanical
//! adds; not implemented in 0.1.0.
//!
//! # Example
//!
//! ```no_run
//! use clienthello::{Extractor, PushOutcome};
//!
//! let mut e = Extractor::new();
//! for datagram in incoming_initials() {
//!     match e.push(&datagram)? {
//!         PushOutcome::Sni(name) => return Ok(name),
//!         PushOutcome::NeedMore => continue,
//!     }
//! }
//! # fn incoming_initials() -> Vec<Vec<u8>> { vec![] }
//! # Ok::<(), clienthello::Error>(())
//! ```

/// Buffered Initial-packet `ClientHello` extraction state.
///
/// Push raw UDP datagrams as they arrive on the wire; each push
/// returns either an extracted SNI or [`PushOutcome::NeedMore`] when
/// the `ClientHello` hasn't fully arrived yet. Push order matters only
/// inasmuch as the `ClientHello` CRYPTO stream's offset metadata is
/// honored: out-of-order datagrams are reassembled internally.
pub struct Extractor {
	// Real fields land alongside the implementation in a follow-up
	// commit. Carrying a private unit-typed marker in the meantime
	// keeps `Extractor::new` from being a unit-struct shorthand and
	// makes the upcoming field addition a non-breaking change.
	_pending_impl: (),
}

impl Extractor {
	/// Build a fresh extractor. Allocates nothing on its own — buffer
	/// growth is bounded by the bytes you feed via [`Self::push`].
	#[must_use]
	pub fn new() -> Self {
		Self { _pending_impl: () }
	}

	/// Feed one UDP datagram into the extractor.
	///
	/// # Errors
	///
	/// See [`Error`] for the full set: malformed long header,
	/// unsupported QUIC version, AEAD decryption failure (typically
	/// the datagram was not an Initial packet for the same connection
	/// the buffer is tracking), CRYPTO frame decode failure,
	/// overlapping CRYPTO ranges, or truncated TLS `ClientHello`.
	///
	/// # Panics
	///
	/// Stub: panics with `unimplemented!` until the implementation
	/// lands in a follow-up commit.
	pub fn push(&mut self, datagram: &[u8]) -> Result<PushOutcome, Error> {
		let _ = datagram;
		unimplemented!("clienthello extraction implementation pending")
	}

	/// Number of bytes buffered across all pushes that contributed to
	/// the `ClientHello` stream. Useful for callers that want to enforce
	/// their own per-session budget alongside the parser.
	///
	/// # Panics
	///
	/// Stub: panics with `unimplemented!` until the implementation
	/// lands in a follow-up commit.
	#[must_use]
	pub fn buffered_bytes(&self) -> usize {
		unimplemented!("clienthello extraction implementation pending")
	}

	/// Number of datagrams pushed since [`Self::new`].
	///
	/// # Panics
	///
	/// Stub: panics with `unimplemented!` until the implementation
	/// lands in a follow-up commit.
	#[must_use]
	pub fn datagrams_seen(&self) -> usize {
		unimplemented!("clienthello extraction implementation pending")
	}
}

impl Default for Extractor {
	fn default() -> Self {
		Self::new()
	}
}

/// Outcome of a single [`Extractor::push`] call.
#[derive(Debug, Clone)]
pub enum PushOutcome {
	/// The TLS `ClientHello` has been fully reassembled and parsed; this
	/// is the SNI. Subsequent pushes return the same value (or another
	/// `Sni` if the caller continues to feed the extractor).
	Sni(String),
	/// More datagrams are needed before the `ClientHello` can be parsed.
	NeedMore,
}

/// Errors that may surface during extraction.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
	/// The datagram is too short, lacks the long-header form bit, or
	/// declares a packet type other than Initial.
	#[error("datagram is not a QUIC long-header Initial packet")]
	NotInitial,
	/// QUIC transport version other than v1 (`0x00000001`).
	#[error("unsupported QUIC version {0:#010x}")]
	UnsupportedVersion(u32),
	/// Long-header field structure malformed (truncated DCID/SCID/
	/// token/length `VarInt`, or a length field that exceeds the
	/// datagram bounds).
	#[error("malformed QUIC long header")]
	HeaderParse,
	/// AEAD decryption failed. Typically means the datagram was not
	/// an Initial packet belonging to the same connection
	/// (different DCID), or the packet was corrupted in transit.
	#[error("AEAD decryption of Initial payload failed")]
	AeadDecrypt,
	/// QUIC frame walker hit a malformed frame, or a frame type that
	/// is not allowed inside an Initial packet (RFC 9000 §17.2.2 only
	/// permits CRYPTO, ACK, PING, PADDING, and `CONNECTION_CLOSE`).
	#[error("malformed or disallowed QUIC frame in Initial payload")]
	FrameDecode,
	/// Two CRYPTO frames cover the same offset range with
	/// non-identical bytes. A well-behaved client never retransmits
	/// Initial CRYPTO ranges with different content; an overlap with
	/// conflicting bytes is treated as adversarial.
	#[error("CRYPTO frames overlap with conflicting bytes")]
	ConflictingOverlap,
	/// TLS `ClientHello` structure malformed, or the `ServerName`
	/// extension is present but contains no `host_name` entry.
	#[error("malformed TLS ClientHello or missing SNI")]
	TlsParse,
}

/// One-shot convenience: extract SNI from a fully-buffered set of
/// Initial datagrams in one call. Equivalent to constructing an
/// [`Extractor`] and feeding each datagram in turn.
///
/// Returns `Ok(None)` if every datagram has been consumed but the
/// `ClientHello` still hasn't fully arrived.
///
/// # Errors
///
/// Forwards [`Error`] from the underlying [`Extractor::push`].
pub fn extract_sni(datagrams: &[&[u8]]) -> Result<Option<String>, Error> {
	let mut e = Extractor::new();
	for d in datagrams {
		if let PushOutcome::Sni(s) = e.push(d)? {
			return Ok(Some(s));
		}
	}
	Ok(None)
}
