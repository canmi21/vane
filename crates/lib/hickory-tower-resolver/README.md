# Hickory Tower Resolver

A `tower::Service<hyper_util::client::legacy::connect::dns::Name>`
adapter around `hickory_resolver::TokioResolver`, so a hickory
resolver can plug into `hyper_util::client::legacy::connect::HttpConnector::new_with_resolver`
and replace hyper's default blocking `GaiResolver`.

## Features

hickory and hyper-util both expose stable APIs but no public bridge
between them — every project that wants async DNS in hyper rewrites
this glue. This crate is that bridge: a small `Service<Name>` impl
plus a [`DnsConfig`] enum (`System` / `Custom(Vec<SocketAddr>)`) for
per-client nameserver overrides, and a `resolve_first_ip` helper for
direct-`IpAddr` callers (e.g. an H3 dial that feeds
`quinn::Endpoint::connect`).

## Example

```rust,no_run
use hickory_tower_resolver::{DnsConfig, HickoryDnsResolver};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

# fn run() -> Result<(), Box<dyn std::error::Error>> {
let resolver = HickoryDnsResolver::build(&DnsConfig::System)?;
let mut connector = HttpConnector::new_with_resolver(resolver);
connector.enforce_http(false);

let client: Client<_, http_body_util::Empty<bytes::Bytes>> =
    Client::builder(TokioExecutor::new()).build(connector);
# let _ = client;
# Ok(())
# }
```

For per-client nameserver overrides, swap `DnsConfig::System` for
`DnsConfig::Custom(vec!["1.1.1.1:53".parse()?, "8.8.8.8:53".parse()?])`.
The `Custom` variant's `Hash` / `PartialEq` impls are order-sensitive,
so two configs with swapped primary / secondary servers occupy
distinct cache slots if you key a client cache by `DnsConfig`.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
