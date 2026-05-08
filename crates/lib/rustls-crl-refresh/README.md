# Rustls CRL Refresh

A process-wide Certificate Revocation List cache and a pair of
refreshable rustls verifiers built on top of it. Lets a long-running
server rotate CRL bytes without rebuilding `Arc<ServerConfig>` /
`Arc<ClientConfig>` — handy when the surrounding code keeps
`Arc`-identity-keyed connection pools and you don't want to churn
them on every CRL refresh.

## Features

- **`CrlCache`** — keyed by source identity (`File(PathBuf)` or
  `Url(String)`). Stores the latest DER bytes plus the parsed
  `nextUpdate`. The cache fetches via a pluggable
  [`CrlFetcher`](trait) — production wires up an HTTP / `tokio::fs`
  fetcher; tests substitute in-memory mocks.
- **`RefreshableClientCertVerifier` / `RefreshableServerCertVerifier`**
  — wrap a `WebPkiClientVerifier` / `WebPkiServerVerifier`
  reconstruction per handshake against the latest cache snapshot.
  Implement rustls's `ClientCertVerifier` / `ServerCertVerifier`
  traits so you slot them straight into a `ServerConfig` /
  `ClientConfig` builder.
- **Per-source failure policy** — each source is registered as
  `tolerate` (keep using last-known bytes when refresh fails) or
  `reject` (fail handshakes once unavailable). Both classes are
  surfaced through `tracing` events.

## Example

```rust,no_run
use std::sync::Arc;
use async_trait::async_trait;
use rustls_crl_refresh::{
    CrlCache, CrlFetchFailure, CrlFetcher, CrlSourceId, RefreshableServerCertVerifier,
};

struct StaticFetcher(Vec<u8>);

#[async_trait]
impl CrlFetcher for StaticFetcher {
    async fn fetch(&self, _src: &CrlSourceId) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }
}

# fn doc_example(roots: Arc<rustls::RootCertStore>, der: Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
let cache = CrlCache::new(Arc::new(StaticFetcher(der)));
let src = CrlSourceId::from_url("https://crl.example/leaf");
cache.ensure_loaded(&[(src.clone(), CrlFetchFailure::Tolerate)])?;

let verifier = RefreshableServerCertVerifier::new(cache, vec![src], roots);
// `verifier: Arc<dyn rustls::client::danger::ServerCertVerifier>` —
// hand to `ClientConfig::builder().with_custom_certificate_verifier(...)`
# Ok(())
# }
```

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
