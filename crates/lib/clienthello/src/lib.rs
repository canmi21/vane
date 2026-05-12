// Modules in this crate carry RFC pseudocode and ASCII protocol
// diagrams in their doc headers; the pedantic `doc_markdown` lint
// would force every protocol identifier inside that pseudocode into
// backticks, which damages readability of the diagrams. Identifiers
// in prose docs still get backticks via per-comment care.
// `aad` (AEAD additional data) and `aead` (the `Aes128Gcm` primitive)
// are domain-canonical names from RFC 5116 / 9001; renaming either
// for the pedantic `similar_names` lint would obscure the crypto.

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
//! Supports **QUIC v1** (transport version `0x00000001`, RFC 9000).
//! QUIC v2 (RFC 9369) is mechanical to add — different initial salt,
//! TLS 1.3 cipher suite — but not yet wired up.
//!
//! # Example
//!
//! ```no_run
//! use clienthello::{Extractor, PushOutcome};
//!
//! # fn incoming_initials() -> Vec<Vec<u8>> { vec![] }
//! fn extract_sni() -> Result<Option<String>, clienthello::Error> {
//!     let mut e = Extractor::new();
//!     for datagram in incoming_initials() {
//!         match e.push(&datagram)? {
//!             PushOutcome::Sni(name) => return Ok(Some(name)),
//!             PushOutcome::NeedMore => continue,
//!         }
//!     }
//!     Ok(None)
//! }
//! ```

mod aead;
mod frame;
mod header;
mod keys;
mod reassemble;
mod tls;

use crate::header::InitialHeader;
use crate::keys::{InitialKeys, derive_client_initial_keys};
use crate::reassemble::CryptoStream;

/// Buffered Initial-packet `ClientHello` extraction state.
///
/// Push raw UDP datagrams as they arrive on the wire; each push
/// returns either an extracted SNI or [`PushOutcome::NeedMore`] when
/// the `ClientHello` hasn't fully arrived yet. Push order is
/// independent of the CRYPTO stream's offset order — out-of-order
/// fragments are reassembled internally.
pub struct Extractor {
	/// Lazily derived from the first datagram's DCID; subsequent pushes
	/// reuse the same keys, so a datagram from a different connection
	/// (different DCID) fails AEAD-decrypt and surfaces as
	/// [`Error::AeadDecrypt`].
	keys: Option<InitialKeys>,
	stream: CryptoStream,
	datagrams_seen: usize,
	/// Cached parsed SNI so subsequent pushes return the same answer
	/// without re-running the parser.
	cached_sni: Option<String>,
}

impl Extractor {
	/// Build a fresh extractor. Allocates nothing on its own — buffer
	/// growth is bounded by the bytes you feed via [`Self::push`].
	#[must_use]
	pub fn new() -> Self {
		Self { keys: None, stream: CryptoStream::new(), datagrams_seen: 0, cached_sni: None }
	}

	/// Feed one UDP datagram into the extractor.
	///
	/// # Errors
	///
	/// See [`Error`] for the full set: malformed long header,
	/// unsupported QUIC version, AEAD decryption failure (typically
	/// the datagram was not an Initial packet for the same connection
	/// the buffer is tracking), CRYPTO frame decode failure,
	/// overlapping CRYPTO ranges with conflicting bytes, or truncated
	/// TLS `ClientHello`.
	pub fn push(&mut self, datagram: &[u8]) -> Result<PushOutcome, Error> {
		self.datagrams_seen += 1;

		if let Some(sni) = &self.cached_sni {
			return Ok(PushOutcome::Sni(sni.clone()));
		}

		let header = InitialHeader::parse(datagram)?;

		// First datagram fixes the keys via DCID; subsequent datagrams
		// reuse them. A different DCID would still parse the long-
		// header but its payload would fail to decrypt under the
		// existing keys, surfacing as `AeadDecrypt`.
		let keys = if let Some(k) = &self.keys {
			k.clone()
		} else {
			let k = derive_client_initial_keys(&header.dcid)?;
			self.keys = Some(k.clone());
			k
		};

		let plaintext = aead::decrypt_initial(datagram, &header, &keys)?;
		for segment in frame::collect_crypto_segments(&plaintext.payload)? {
			self.stream.push(segment.offset, &segment.data)?;
		}

		match self.stream.contiguous_prefix() {
			Some(prefix) => match tls::try_extract_sni(prefix)? {
				Some(sni) => {
					self.cached_sni = Some(sni.clone());
					Ok(PushOutcome::Sni(sni))
				}
				None => Ok(PushOutcome::NeedMore),
			},
			None => Ok(PushOutcome::NeedMore),
		}
	}

	/// Number of bytes buffered in the CRYPTO reassembly stream.
	/// Useful for callers that want to enforce their own per-session
	/// budget alongside the parser.
	#[must_use]
	pub fn buffered_bytes(&self) -> usize {
		self.stream.total_bytes()
	}

	/// Number of datagrams pushed since [`Self::new`].
	#[must_use]
	pub fn datagrams_seen(&self) -> usize {
		self.datagrams_seen
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
	///
	/// **Invariant**: the returned string is ASCII-lowercased. SNI is
	/// case-insensitive (RFC 6066 §3); normalization happens at the
	/// parser so downstream comparators (e.g. `tls.sni` predicates,
	/// `CertStore::by_sni`) can rely on lowercase keys.
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
