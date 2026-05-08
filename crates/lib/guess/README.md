# Guess

Wire-protocol classifier for TCP / TLS streams. Feed the first bytes
of a freshly accepted connection and get back one of:

- **TLS ClientHello** (parsed via `rustls::server::Acceptor`; SNI and
  ALPN extracted)
- **HTTP/2 connection preface** (RFC 7540 §3.5)
- **HTTP/1** (request line with `HTTP/1.0` or `HTTP/1.1` version
  marker)
- **Unknown** (every detector ruled itself out — further reads will
  not change the outcome)

The cascade is three-state: when _some_ detector wants more bytes
before committing, `classify` returns `detected = None` so the caller
knows to read more (up to `MAX_PEEK_BYTES`) and call again.

## Example

```rust
use guess::{DetectedProtocol, MAX_PEEK_BYTES, classify};

# fn handle(stream_bytes: &[u8]) {
let result = classify(stream_bytes);
match result.detected {
    Some(DetectedProtocol::TlsClientHello) => {
        let tls = result.tls.unwrap();
        println!("SNI: {:?}, ALPN: {:?}", tls.sni, tls.alpn);
    }
    Some(DetectedProtocol::Http2Preface) => println!("h2"),
    Some(DetectedProtocol::Http1) => println!("h1"),
    Some(DetectedProtocol::Unknown) => println!("opaque L4"),
    None => println!("read more bytes (up to {MAX_PEEK_BYTES})"),
    _ => {}
}
# }
```

## Features

- `classify` (default) — pulls in `rustls` (for the TLS parse) and
  `memchr` (for the HTTP/1 scan), and exposes `classify`. Disable to
  get only the result types — useful when a downstream crate wants
  to _describe_ a peek without performing one:
  ```toml
  guess = { version = "0.2", default-features = false }
  ```

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
