# OCSP Staple

Build OCSP requests, parse OCSP responses, and extract OCSP responder
URLs from a certificate's Authority Information Access (AIA)
extension. With the `fetch` feature, also performs an async HTTP/1.1
POST against the responder via hyper.

## Features

The crate is structured in three layers:

- **Pure functions on cert DER** (always compiled) — `extract_ocsp_url`,
  `build_ocsp_request`, `parse_ocsp_response`. No IO, unit-testable
  in isolation.
- **One async transport function** (`fetch` feature) — `fetch_ocsp`.
- **One convenience wrapper** (`fetch` feature) — `fetch_ocsp_for_cert`
  runs the whole pipeline (extract → build → fetch → parse).

## Transport

Production CAs ship HTTP-only OCSP responders, and OCSP responses
are independently signed. This crate enforces HTTP-only: HTTPS URLs
surface as `OcspError::HttpsNotSupported`. Pre-fetched responses for
HTTPS-only responders should be delivered through other channels.

## Example

```rust
use std::time::Duration;
use ocsp_staple::{FETCH_TIMEOUT, fetch_ocsp_for_cert};

# async fn run(leaf_der: &[u8], issuer_der: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
let staple = fetch_ocsp_for_cert(leaf_der, issuer_der, FETCH_TIMEOUT).await?;
// `staple.staple` is the DER blob to ship to clients.
// `staple.next_update` is the wall-clock deadline for the next refresh.
# Ok(())
# }
```

The pure-function path is available without any features:

```rust
use ocsp_staple::{build_ocsp_request, extract_ocsp_url, parse_ocsp_response};

# fn run(leaf_der: &[u8], issuer_der: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
let url = extract_ocsp_url(leaf_der)?;
let req = build_ocsp_request(leaf_der, issuer_der)?;
// ... POST `req` to `url` via your own transport ...
# let resp_bytes: Vec<u8> = vec![];
let staple = parse_ocsp_response(&resp_bytes)?;
# Ok(())
# }
```

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
