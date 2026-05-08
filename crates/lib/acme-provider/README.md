# ACME Provider

The `DnsProvider` trait used by an ACME client to satisfy a DNS-01
challenge: set a TXT record, wait for it to propagate, delete it
after validation.

## Features

The crate ships **trait + error only** by default. Concrete
provider implementations live behind feature flags so a downstream
build only pulls in transports for the providers it actually uses.

| Provider   | Feature      | Status   |
| ---------- | ------------ | -------- |
| Cloudflare | `cloudflare` | Built-in |

(Add a row when contributing a new provider.)

## Example

```rust,no_run
use std::sync::Arc;
use std::time::Duration;
use acme_provider::{DnsProvider, DnsProviderError};

# async fn issue(name: &str, key_authorization: &str, dns: Arc<dyn DnsProvider>) -> Result<(), DnsProviderError> {
dns.set_txt(name, key_authorization).await?;
dns.wait_propagated(name, key_authorization, Duration::from_secs(120)).await?;
// ... call your ACME client's challenge-ready / finalize / fetch-cert ...
dns.delete_txt(name).await?;
# Ok(())
# }
```

## Cloudflare provider

Enable the `cloudflare` feature. The provider authenticates with a
Cloudflare API Token scoped to "Zone DNS Edit" — the rule-side
config holds only the env-var name, never the token itself.

```rust,ignore
use acme_provider::cloudflare::{CloudflareConfig, CloudflareDnsProvider};

let cfg = CloudflareConfig {
    api_token_env: "CF_API_TOKEN".to_owned(),
    zone_id: None, // auto-detect by walking labels
};
let provider = CloudflareDnsProvider::from_config(&cfg)?;
```

`wait_propagated` polls a small fixed pool of public recursive
resolvers (`1.1.1.1`, `8.8.8.8`) — observing the TXT through a
public resolver is a high-confidence proxy for what the CA validator
will see.

> **Note on TLS init:** `reqwest` 0.12 with `rustls-tls-native-roots-no-provider`
> requires a rustls crypto provider be installed before
> `Client::build` runs. Pick one (`rustls::crypto::aws_lc_rs::default_provider().install_default()`
> or `ring`) at host boot.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
