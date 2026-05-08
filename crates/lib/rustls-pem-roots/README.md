# Rustls PEM Roots

Build a `rustls::RootCertStore` from any combination of explicit file
paths and a roots directory, deduplicating identical certificates by
full DER bytes and distinguishing "path unreadable" / "PEM
unparseable" / "no roots found" as separate error variants.

`rustls-pemfile` only reaches the single-file parse layer; a "drop a
folder of CA files in here" loader is the part every operator-facing
TLS service ends up writing by hand.

## Example

```rust,no_run
use std::path::PathBuf;
use rustls_pem_roots::load;

# fn run() -> Result<(), rustls_pem_roots::Error> {
let store = load(
    &[PathBuf::from("/etc/ssl/internal-ca.pem")],
    Some(std::path::Path::new("/etc/ssl/cas.d")),
)?;

let _verifier = rustls::server::WebPkiClientVerifier::builder(store.into())
    .build()
    .expect("build verifier");
# Ok(())
# }
```

`load` calls `add_pem_file` for each explicit path then `add_pem_dir`
for the directory. If you need to compose multiple sources into a
shared store while tracking dedup state across calls, those two
functions are also exposed and take a caller-managed `&mut HashSet<Vec<u8>>`.

## Directory semantics

`add_pem_dir` reads all entries of `dir` and processes the ones whose
extension is `.pem`; any other extension is silently skipped, and
subdirectories are not recursed. This matches how Caddy / nginx /
HAProxy treat their CA-bundle drop-in directories.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
