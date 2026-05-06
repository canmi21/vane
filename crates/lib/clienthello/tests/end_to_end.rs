//! End-to-end pipeline test: build a synthetic QUIC v1 Initial
//! datagram carrying a `ClientHello` with a chosen SNI; feed it to
//! `Extractor` and recover the SNI.
//!
//! Self-consistency only — the encryption side uses the same RFC 9001
//! algorithms the decrypt side uses, so this test catches any
//! mismatched offset / length / endianness wiring inside the crate
//! but does not independently verify the RFC. The RFC 9001
//! Appendix A.1 known-answer vectors live in `keys::tests` (lib).

use clienthello::{Extractor, PushOutcome, extract_sni};

mod helpers;
use helpers::build_initial_datagram_with_sni;

#[test]
fn extractor_recovers_sni_from_single_initial_datagram() {
	let dcid = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x11, 0x22];
	let datagram = build_initial_datagram_with_sni(&dcid, "edge.example.test");

	let mut e = Extractor::new();
	match e.push(&datagram).expect("push") {
		PushOutcome::Sni(sni) => assert_eq!(sni, "edge.example.test"),
		PushOutcome::NeedMore => panic!("single datagram should carry the entire ClientHello"),
	}
	assert_eq!(e.datagrams_seen(), 1);
}

#[test]
fn extract_sni_one_shot_helper_recovers_sni() {
	let dcid = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
	let datagram = build_initial_datagram_with_sni(&dcid, "api.example.org");
	let result = extract_sni(&[&datagram]).expect("extract");
	assert_eq!(result.as_deref(), Some("api.example.org"));
}

#[test]
fn fully_buffered_no_clienthello_returns_none() {
	// Empty input — extract_sni reports no SNI yet.
	let result = extract_sni(&[]).expect("extract empty");
	assert!(result.is_none());
}

#[test]
fn extractor_caches_sni_across_subsequent_pushes() {
	let dcid = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80];
	let datagram = build_initial_datagram_with_sni(&dcid, "cache.example");

	let mut e = Extractor::new();
	let first = e.push(&datagram).expect("push 1");
	let second = e.push(&datagram).expect("push 2");
	let (PushOutcome::Sni(s1), PushOutcome::Sni(s2)) = (first, second) else {
		panic!("expected SNI on both pushes");
	};
	assert_eq!(s1, s2);
	assert_eq!(s1, "cache.example");
}
