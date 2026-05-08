# OCSP Mock Responder

In-process mock OCSP responder for integration tests.
`MockOcspResponder::start(issuer_der)` spins up a hyper HTTP/1.1
server on an ephemeral port; incoming `application/ocsp-request`
POSTs are answered with a configurable `Good` / `Revoked` /
`TryLater` status mirroring what a real CA responder would return.

The server is a fixture, not a full CA: the response carries a
**placeholder signature**. OCSP-stapling consumers that treat the
staple as opaque bytes (rustls's `CertifiedKey.ocsp` path; OCSP-
aware client verifiers that trust the responder via TLS) work
unchanged. Tests that re-validate the responder's signature need a
different fixture.

## Example

```rust
use std::time::Duration;
use ocsp_mock_responder::{MockOcspResponder, OcspMockStatus};

# async fn run(issuer_der: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
let responder = MockOcspResponder::start(issuer_der).await?;
let url = responder.url();
// Point your code-under-test at `url`. Switch responses at will:
responder.set_status(OcspMockStatus::good_for(Duration::from_secs(3600)));
// ... drive the test ...
responder.set_status(OcspMockStatus::Revoked);
// ... assert your code reacts as expected ...
println!("OCSP responder hits: {}", responder.hits());
// Drop `responder` to stop the server.
# Ok(())
# }
```

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
