# ClientHello

Extract the TLS Server Name Indication (SNI) from a QUIC client's
Initial datagrams without performing a full QUIC handshake.

QUIC Initial packets carry the TLS ClientHello in CRYPTO frames,
AEAD-encrypted with keys derived from the client's chosen
**Destination Connection ID** (RFC 9001 §5.2). Any party with the
DCID can decrypt the Initial — no negotiated secret material is
involved at this layer. This crate exposes that primitive: feed it
raw datagrams as they arrive, get back the SNI when enough of the
ClientHello has been seen.

## Features

- SNI-aware UDP load balancers
- Observability probes for QUIC traffic
- Any system that needs to route QUIC connections by server name without terminating them

## Example

```rust
use clienthello::{Extractor, PushOutcome};

let mut e = Extractor::new();
for datagram in incoming_initials() {
    match e.push(datagram)? {
        PushOutcome::Sni(name) => return Ok(name),
        PushOutcome::NeedMore => continue,
    }
}
# fn incoming_initials() -> Vec<Vec<u8>> { vec![] }
# Ok::<(), clienthello::Error>(())
```

## Versions

Currently supports **QUIC v1** (transport version `0x00000001`,
RFC 9000). v2 (RFC 9369) salt and TLS 1.3 cipher-suite alternates are
mechanical adds; not implemented in 0.1.0.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
