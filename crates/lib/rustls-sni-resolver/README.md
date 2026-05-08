# Rustls SNI Resolver

A minimal `ResolvesServerCert` implementation backed by
`{ by_sni: HashMap<String, Arc<E>>, default: Option<Arc<E>> }`, with
the whole struct designed to live behind an `Arc<ArcSwap<_>>` so a
config reload is one atomic pointer swap.

`E` is generic over the [`EntryKey`] trait, so callers can attach
their own per-cert state (expiry timestamps, OCSP staple handles,
ACME order IDs, …) without forking the resolver.

rustls's built-in `ResolvesServerCertUsingSni` returns `None` on
unmatched SNI with no built-in fallback hook — every operator-facing
TLS service ends up writing this small "with a default" variant by
hand.

## Example

```rust,no_run
use std::sync::Arc;
use arc_swap::ArcSwap;
use rustls_sni_resolver::{CertStore, EntryKey, Resolver};

#[derive(Debug)]
struct MyEntry {
    key: Arc<rustls::sign::CertifiedKey>,
    not_after: std::time::SystemTime,
}

impl EntryKey for MyEntry {
    fn key(&self) -> Arc<rustls::sign::CertifiedKey> {
        Arc::clone(&self.key)
    }
}

# fn run(api_entry: Arc<MyEntry>, default_entry: Arc<MyEntry>) {
let mut store: CertStore<MyEntry> = CertStore::new();
store.by_sni.insert("api.example.com".into(), api_entry);
store.default = Some(default_entry);

let store = Arc::new(ArcSwap::from_pointee(store));
let resolver: Arc<dyn rustls::server::ResolvesServerCert> =
    Arc::new(Resolver::new(store.clone()));

// Later, on reload, swap atomically:
let mut fresh: CertStore<MyEntry> = CertStore::new();
// ... populate fresh ...
store.store(Arc::new(fresh));
# }
```

## Lookup semantics

`CertStore::lookup(Option<&str>)` returns:

- the entry under the matching SNI key, if one exists;
- otherwise the `default` entry, if one is set;
- otherwise `None`.

Because rustls already ASCII-lowercases the `server_name` per RFC 6066
§ 3, populators should also store `by_sni` keys in lowercase. This
crate does **not** lowercase on insert — callers own that invariant
(typical populators read keys from configuration that has already been
normalized at parse time).

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
