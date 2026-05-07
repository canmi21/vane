# vane-clienthello

Source: [`crates/lib/clienthello/`](../../crates/lib/clienthello/).

Standalone, independently publishable QUIC ClientHello parser. Pure Rust, no `vane-*` dependency, MIT-licensed. Shaped to be adoptable by any project needing the same primitive.

## Why it exists

Vane needs to extract SNI from a QUIC Initial packet without terminating the connection — the QUIC SNI passthrough path in [`engine.md` § _Multi-packet peek_](engine.md#multi-packet-peek). Initial packet payloads are encrypted with a key derived from the connection's initial Destination Connection ID (RFC 9001 §5.2), so any party with the DCID can decrypt — no secret material required.

Reaching into `quinn-proto` / `rustls` internals to do this is not feasible:

- `quinn_proto::initial_keys` and the wider Initial-keys derivation surface are `pub(crate)` — not callable from outside `quinn-proto`.
- Hand-rolling the four labels' worth of HKDF-Expand-Label per RFC 9001 §5.1 / §5.2 is ~50 LoC of standards-fixed code: salt and labels are pinned to the RFC and will not drift across `quinn` releases.
- Avoiding `rustls::server::Acceptor` keeps the crate independent of rustls's evolving session API and lets it stay deployable without a TLS termination stack on the consumer side.

## What it does

- QUIC long-header parsing per RFC 9000 §17.2 (Initial-only). Source: `header.rs`.
- HKDF-Expand-Label key derivation per RFC 9001 §5.1 / §5.2 against the v1 initial salt. Source: `keys.rs`.
- AES-128-ECB header protection per RFC 9001 §5.4 + AES-128-GCM payload AEAD per RFC 9001 §5.3. Source: `aead.rs`.
- QUIC frame walking (CRYPTO / PADDING / ACK / PING; any other frame type in an Initial packet is a protocol violation and drops the pending session). Source: `frame.rs`.
- Offset-keyed CRYPTO byte-stream reassembly across multiple Initial datagrams. Source: `reassemble.rs`.
- TLS ClientHello SNI extension extraction per RFC 6066 §3 — the ClientHello arrives as raw handshake bytes (QUIC carries no TLS record header, parser starts from the HandshakeMessage). Source: `tls.rs`.

## Crypto backend

RustCrypto: `aes-gcm`, `aes`, `ctr`, `hkdf`, `sha2`, `subtle`. Pure Rust, no C deps, friendly to downstream consumers.

## API

`extract` returns `Some(sni)` when a complete ClientHello has been observed and SNI parsed; `None` otherwise (more datagrams needed). Errors during decryption or parsing (malformed packet, version mismatch) drop the pending session — the crate does not attempt recovery.

Source: `lib.rs`.

## QUIC v2

```rust
// TODO(quic-v2): mechanical to add (different initial salt + TLS 1.3
// cipher suite per RFC 9369). Not implemented in 0.1.0.
```

## Tests

`crates/lib/clienthello/tests/end_to_end.rs` plus `helpers/`. Self-contained — tests synthesize representative QUIC Initial packets (single-datagram and multi-datagram) and verify SNI extraction.

## CLAUDE.md

`crates/lib/CLAUDE.md` documents the conventions for the entire `crates/lib/` family: independently publishable, no `vane-*` dependency, must compile and test on its own.
