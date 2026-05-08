# Rustls Native Roots Cache

Process-wide cache for rustls's native trust store, with bounded
retry on macOS Security framework transient errors and sticky
failure semantics.

`rustls_native_certs::load_native_certs` reaches into the OS keychain
(Security framework on macOS, NSS / OpenSSL stores on Linux). On
macOS the underlying `Sec*` APIs are not concurrency-safe under load
— multiple threads calling them in parallel can return `errSecIO`
(-36). Daemons that build many distinct rustls `ClientConfig`s (one
per upstream-TLS fingerprint, for instance) hit this whenever a
reload introduces a handful of new fingerprints concurrently.

This crate caches the trust store once per process behind an
`Arc<rustls::RootCertStore>`. Concurrent first calls are serialised
by `OnceLock`; subsequent calls are a cheap `Arc::clone`. The first
load retries on transient failure with a small backoff (Apple
documents `errSecIO` as recoverable), and the outcome is sticky —
the cached error re-yields on subsequent calls so operators see
consistent behaviour and can restart the process to retry.

## Example

```rust
use rustls_native_roots_cache::native_roots;

# fn run() -> Result<(), Box<dyn std::error::Error>> {
let roots = native_roots()?;
let mut config = rustls::ClientConfig::builder()
    .with_root_certificates(rustls::RootCertStore::clone(&roots))
    .with_no_client_auth();
# Ok(())
# }
```

`warm_native_roots()` eagerly performs the first load — useful when a
daemon's boot path wants to surface trust-store failure before the
first TLS handshake.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
