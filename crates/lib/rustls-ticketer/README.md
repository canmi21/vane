# Rustls Ticketer

A tiny, idempotent installer for rustls's process-wide TLS session
ticketer. Useful for daemons and test harnesses that bind many
`ServerConfig`s and want them to share a single ticketer Arc — the
self-rolling RFC 5077 rotator from rustls already handles key
rotation and ticket lifetime; this crate just makes "install once,
read many" trivial.

## Features

Pick exactly one crypto backend — same choice as the rest of your
rustls stack:

- `aws-lc-rs` (recommended)
- `ring`

Both rely on `rustls::server::Ticketer::new()` which constructs an
`Arc<rustls::TicketRotator>` (AES-256-CBC + HMAC-SHA256, 6-hour
rotation, 12-hour ticket lifetime).

## Example

```rust,no_run
use rustls_ticketer::{default_ticketer, install_default_ticketer};

# fn boot() -> Result<(), rustls::Error> {
// Call once at boot — must be after rustls's `CryptoProvider`
// install, since the backend's RNG fuels the initial key.
install_default_ticketer()?;

// Each listener reads the same Arc:
let mut server_cfg = rustls::ServerConfig::builder()
    .with_no_client_auth()
    .with_cert_resolver(unimplemented!()); // your resolver here
if let Some(t) = default_ticketer() {
    server_cfg.ticketer = t;
}
# Ok(())
# }
```

`install_default_ticketer` is idempotent — a second call after a
successful install is a no-op and returns `Ok(())`. Test harnesses
and a daemon's `main` can both invoke without coordination.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
