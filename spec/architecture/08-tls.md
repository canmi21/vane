# TLS

## TLS library: rustls only

`vane` commits to [`rustls`](https://crates.io/crates/rustls) as its single TLS library. The following are **non-dependencies by policy** — a PR introducing any of them (direct dep, or a default-feature of an indirect dep) is rejected:

- **[`native-tls`](https://crates.io/crates/native-tls)** — wraps the host OS's TLS library (SChannel / Secure Transport / OpenSSL). A competitor to rustls.
- **`openssl`, `openssl-sys`** — FFI to OpenSSL.
- **`boring`, `boring-sys`** — FFI to BoringSSL.
- **`hyper-tls`** — hyper connector built on `native-tls`. We use `hyper-rustls` instead.

### Confusingly-named crate that we _do_ use

- **[`rustls-native-certs`](https://crates.io/crates/rustls-native-certs)** — **pure Rust**, despite the name. It loads the **host's root-CA bundle** (via `security-framework` on macOS, filesystem probe via `openssl-probe` on Linux — the probe is a filesystem lookup, not an OpenSSL dep) and hands those roots to rustls. It is an **ally** of rustls, not a competitor. We depend on it; it is listed in the `vane-engine` deps in `16-crate-layout.md`.

The naming is unfortunate. To disambiguate in reviews: if the crate name starts with `rustls-` or is a rustls-flavored feature on another crate (e.g., `reqwest`'s `rustls-tls`, `hickory-resolver`'s `tls-aws-lc-rs` / `tls-ring` / `https-aws-lc-rs` features), it is an ally. If it starts with `native-tls` / `openssl` / `boring` / `hyper-tls`, it is a competitor and banned.

### Enforcement

When pulling a crate that offers multiple TLS backends (`reqwest`, `ureq`, `hyper`, `hickory-resolver`, etc.), the `Cargo.toml` entry must carry `default-features = false` and explicitly select a rustls-flavored feature. "Accidentally defaults to native-tls" is a regression, not a shrug.

This is orthogonal to the `aws-lc-rs` ↔ `ring` crypto-provider choice in `16-crate-layout.md` § _Crypto backend_. Both providers are rustls-internal; the policy here is about never pulling a _different_ TLS library in front of rustls.

A CI check (post-MVP) asserts `! cargo tree --workspace | grep -q 'openssl-sys '` to catch regressions mechanically. See `script/check-no-openssl.sh` in `16-crate-layout.md` § _CI orchestration shape_.

## Principle: two orthogonal dimensions

TLS policy in `vane` is **two independent dimensions**, not a single strictness toggle:

- **Client ↔ vaned** — determined by listener + rule configuration. Binding only `:443` forces HTTPS; binding `:80` accepts plain HTTP; combining `:80` with a `redirect_https` rule produces HTTP-upgrade-to-HTTPS.
- **vaned ↔ upstream** — determined by the upstream's protocol choice (`HttpUpstream::Tcp.tls: Option<UpstreamTls>`) and, when TLS is used, its verification mode (`Full` vs `Skip`).

These compose into a 6-cell configuration matrix; `vane` accepts all combinations. No architecture-level "禁止" policy — users pick their own security posture, and `vane compile --dry-run` plus external lints can enforce organizational policies on top.

```
                         Upstream HTTP     Upstream HTTPS Full     Upstream HTTPS Skip
Listener :80 (HTTP)       plaintext         start TLS to origin     start TLS, no verify
Listener :443 (HTTPS)     client-TLS→plain  end-to-end TLS verified end-to-end TLS no-verify
```

## Scenarios

| Scenario                               | Decrypt?           | Cert needed?                 | Where                 |
| -------------------------------------- | ------------------ | ---------------------------- | --------------------- |
| Pure L4 TCP forward                    | No                 | No                           | —                     |
| L4 SNI-based routing (peek only)       | No                 | No                           | Listener peek         |
| L4 → L7 upgrade (need HTTP visibility) | Yes                | Yes (server cert)            | On upgrade            |
| Upstream TLS `VerifyMode::Full`        | Yes                | Root CA (plus optional mTLS) | On upstream connect   |
| Upstream TLS `VerifyMode::Skip`        | Yes (encrypt only) | None                         | On upstream connect   |
| mTLS listener (`ClientAuth::Require`)  | Yes                | Yes (client trust store)     | On listener handshake |

---

## Listener-side TLS

### SNI peek (L4)

A built-in L4 middleware reads the ClientHello from the peek buffer and populates `ctx.tls.sni` and `ctx.tls.alpn` **without decrypting**. Uses [`rustls::server::Acceptor`](https://docs.rs/rustls/latest/rustls/server/struct.Acceptor.html): feed bytes via `read_tls`, call `accept()` to get an `Accepted` that exposes `client_hello() -> ClientHello<'_>`. No handshake yet — on the L4 routing path we **abort** here (drop the `Accepted`); on the TLS termination path (see below) we **continue** via `Accepted::into_connection(config)` to start the handshake. Same `Acceptor` serves both roles.

Once populated, L4 predicates match on `tls.sni`, enabling SNI-based L4 forward to different upstreams per tenant — the standard pattern for TLS-passthrough load balancing.

#### SNI normalization invariant

DNS names are case-insensitive (RFC 4343); some clients send `API.example.COM` in the ClientHello, most send `api.example.com`. To keep one comparison path on the hot side, the SNI string is **ASCII-lowercased at every ingress boundary** and the invariant "`ctx.tls.sni` is lowercase" is maintained system-wide:

- The SNI peek middleware lowercases the parsed `server_name` before writing `ctx.tls.sni`.
- `VaneCertResolver::resolve` lowercases `client_hello.server_name()` before the `lookup` call.
- `CertStore::by_sni` keys are stored lowercase at populator-time; populators that read user-provided cert hostnames call `to_ascii_lowercase()` on insert.
- The predicate compiler (`lower` pass in `02-flow.md`) rejects `tls.sni { equals/prefix/suffix/... "X" }` literals that contain uppercase ASCII with a clear error: `"tls.sni literals must be lowercase — saw 'Api.example.com' at rules/30-api.json:14"`. This keeps hot-path comparison byte-for-byte; no `eq_ignore_ascii_case` shim.

Non-ASCII hostnames follow IDNA: clients are expected to send punycode (`xn--`) in SNI; `vane` does not attempt U-label → A-label conversion. Cert files loaded via `StaticCertPopulator` must list hostnames in punycode form.

### TLS termination (L4 → L7 upgrade)

When a FlowGraph path requires HTTP inspection on a TLS-wrapped connection:

```
TCP stream
  ↓ peek ClientHello (L4, no decrypt)
  ↓ SNI-based cert lookup   → rustls::ServerConfig
  ↓ rustls handshake        → plaintext AsyncRead + AsyncWrite; ctx.tls populated
  ↓ ALPN dispatch           → hyper (H1/H2) or h3 (H3); ctx.http_version populated
  ↓ parse                   → Request
```

### Version, cipher, ALPN policy

- **TLS versions** — 1.3 preferred, 1.2 accepted, 1.0 / 1.1 / SSL v1 / v2 / v3 all rejected. `vane` is a new-era engine and carries no legacy protocol baggage.
- **Cipher suites** — rustls defaults. No custom tuning; safe choice maintained upstream by the rustls project.
- **ALPN** — listener advertises based on `kind`:
  - `Http` → `h2`, `http/1.1`
  - QUIC listener → `h3`
  - `Auto` → all applicable, peer picks

### Cert resolver and rotation

The resolver implements rustls's `ResolvesServerCert`:

```rust
impl rustls::server::ResolvesServerCert for VaneCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let store = self.store.load();
        // Explicit lookup-then-fallback. We do **not** delegate to
        // `rustls::server::ResolvesServerCertUsingSni` because that
        // resolver returns `None` (handshake failure) on unmatched
        // SNI, with no built-in fallback hook. The CertStore's `default`
        // field is the explicit no-SNI fallback (default: reject; see
        // _Fallback behavior_ below for opt-in).
        if let Some(sni) = client_hello.server_name()
            && let Some(found) = store.by_sni.get(sni)
        {
            return Some(Arc::clone(&found.key));
        }
        store.default.as_ref().map(|d| Arc::clone(&d.key))
    }
}

pub struct VaneCertResolver {
    store: Arc<ArcSwap<CertStore>>,
}

pub struct CertStore {
    by_sni:  HashMap<String, Arc<CertEntry>>,
    default: Option<Arc<CertEntry>>,    // optional no-SNI fallback
}
// SNI keys are sourced from the rule layer's explicit `tls.sni` field
// (see 14-presets.md / 09-config.md `RawRule.tls`), **not** parsed from
// the cert's SAN/CN. Parsing certs at compile time would couple the
// config layer to an x509 dependency and silently bind cert content
// changes to routing changes (a re-issued cert with a different SAN
// would silently rewrite the SNI map). Operators name the SNI they
// route on; the cert's contents must agree, but the routing key is
// the explicit field.
//
// A rule whose `tls` has no `sni` field becomes the listener's
// `default` cert (one default per listener, enforced at lower).

pub struct CertEntry {
    pub key:              Arc<CertifiedKey>,   // CertifiedKey.ocsp carries OCSP stapling data
    pub not_after:        SystemTime,
    pub ocsp_next_update: Option<Instant>,
}
```

Rotation is an `ArcSwap` replacement of the inner `CertStore`. Live TLS connections keep their handshake-time cert; only **new handshakes** see the new cert. TLS protocol does not permit mid-connection cert change — this is not a limitation of `vane`, it is the protocol. ("How does a live connection observe a rotated cert?" — it does not.)

### Fallback behavior

| Situation           | Behavior                                           |
| ------------------- | -------------------------------------------------- |
| Client sends no SNI | Use `default_cert` if configured; otherwise reject |
| SNI not in store    | Same                                               |
| No cert resolved    | TLS handshake fails; TCP connection closes         |

Default: **reject**. Opt-in fallback via `default_cert` in `config.json`. Silent mismatch (presenting a cert for the wrong domain) is worse than an explicit TLS error — clients detect the latter immediately; the former may leak to application layer before detection.

### Cert populators

Certs are loaded by `CertPopulator` implementations:

```rust
pub trait CertPopulator: Send + Sync {
    async fn initial_store(&self) -> Result<CertStore>;
    async fn refresh(&self, current: &CertStore) -> Result<Option<CertStore>>;
}
```

The architecture supports **multiple populators simultaneously** — public-facing domains via ACME, internal domains via static files, both populating the same store keyed by SNI.

Built-in implementations:

- **`StaticCertPopulator`** — loads cert/key files from configured paths. Optional OCSP response file paths, or optional OCSP fetch from the cert's AIA URL on refresh.
- **`ManagedCertPopulator`** (integrated LazyCert) — ACME / Let's Encrypt automatic issuance and renewal via [`instant-acme`](https://crates.io/crates/instant-acme) (pure-Rust RFC 8555 client, rustls-compatible). OCSP fetched from the cert's AIA URL automatically. See "ACME challenge modes" below for DNS-01 / HTTP-01 handling.

`refresh()` runs periodically (default: every 5 minutes). Each populator decides what is stale — near-expiry cert, expired OCSP response. If stale, return a new `CertStore` for `ArcSwap` to install.

### OCSP stapling

OCSP stapling is carried inline on the cert: `CertifiedKey.ocsp: Option<Vec<u8>>`. During handshake, rustls staples these bytes to the ServerHello automatically.

The populator is responsible for keeping OCSP fresh. OCSP responses typically validate for 4–7 days; refresh daily.

### Session ticket rotation

Session tickets let clients resume TLS sessions without a full handshake. The server encrypts session state with a key that must rotate periodically — a leaked key compromises all sessions encrypted with it.

Daemon-level manager:

```rust
pub struct TicketKeyManager {
    current:         ArcSwap<TicketKey>,
    previous:        ArcSwap<Option<TicketKey>>,   // accept tickets from previous key during transition
    rotation_period: Duration,                      // default 24h, configurable
}

impl rustls::server::ProducesTickets for TicketKeyManager { /* ... */ }
```

Background task rotates keys: generate new current, current → previous, drop old previous. All `ServerConfig`s share a single `Arc<TicketKeyManager>` — daemon-wide consistency.

### TLS 1.3 0-RTT (early data)

0-RTT lets clients send application data in the first flight, saving one round-trip. **Trade-off**: the data is not replay-protected — attackers can replay captured 0-RTT packets against idempotent endpoints.

Two-level opt-in:

```rust
pub struct ListenerTlsConfig {
    pub enable_0rtt: bool,    // default false
}

pub struct Rule {
    // ... other fields ...
    pub allow_0rtt: bool,     // default false
}
```

Runtime flow:

```
Client sends 0-RTT data
  ↓ rustls decrypts, exposes as "early data"
  ↓ we parse Request, walk FlowGraph to matched rule
  ↓ rule.allow_0rtt is false?
      ├─ yes → reject early data, rustls sends HRR, client retries with full handshake
      └─ no  → accept, proceed normally
```

**Compile-time constraint**: `allow_0rtt: true` is only legal on rules whose match predicates include a method constraint restricted to idempotent methods (GET / HEAD / OPTIONS). Non-idempotent rules with `allow_0rtt: true` are a compile error.

### Client certificate verification (mTLS on listener)

Listener may request or require client certificates:

```rust
pub enum ClientAuth {
    None,                             // default: don't request client cert
    Request {                         // request, don't require; log if provided
        trust_store: Arc<ArcSwap<ClientTrustStore>>,
    },
    Require {                         // require; fail handshake if missing or invalid
        trust_store: Arc<ArcSwap<ClientTrustStore>>,
    },
}

pub struct ClientTrustStore {
    pub cas:  RootCertStore,
    pub crls: Vec<CertificateRevocationList>,    // optional CRL for revocation checks
}
```

Verified client certs populate `ctx.tls.peer_cert`. L7 middleware can match on it:

```json
{ "tls.peer_cert.subject_cn": { "equals": "admin@example.com" } }
```

`ClientTrustStore`'s `ArcSwap` rotation is symmetric to `CertStore` — same pattern, same mental model.

---

## Upstream-side TLS

### `UpstreamTls`

```rust
pub struct UpstreamTls {
    pub root_ca:     RootCaSource,
    pub client_cert: Option<Arc<CertifiedKey>>,   // optional mTLS client cert (Arc-shared; see below)
    pub crls:        Vec<CrlSource>,              // optional CRL list
    pub verify_mode: VerifyMode,                  // default Full
    pub alpn:        Option<Vec<String>>,         // default: derived from HttpUpstream.version
    pub sni:         Option<String>,              // override; default: derived from addr hostname
}

pub enum RootCaSource {
    System,                   // via rustls-native-certs
    Bundle(PathBuf),          // custom PEM bundle
}

pub enum VerifyMode {
    /// Full validation: chain + hostname + CRL (if provided) + OCSP (if stapled)
    Full,

    /// Skip validation entirely. Still runs TLS 1.2/1.3 handshake (encrypted connection),
    /// but does not verify the upstream's identity. Use for self-signed certs or dev environments.
    Skip,
}

pub enum CrlSource {
    File(PathBuf),
    Url(String),              // fetched periodically
}
```

**`Skip` does not mean "no encryption"**: the TLS handshake still runs; traffic is still encrypted; only cert identity verification is skipped. The connection is still protected against passive eavesdropping, just not against active MITM.

### ClientConfig fingerprint and caching

`rustls::ClientConfig` construction is expensive. Daemon caches configs by fingerprint:

```rust
daemon.tls_config_cache: HashMap<TlsConfigFingerprint, Arc<ClientConfig>>

TlsConfigFingerprint = hash(
    root_ca_source,     // System: constant tag; Bundle(path): path string
    client_cert,        // hash of CertifiedKey bytes (cert DER + key DER)
    crls,               // hash of CRL *sources* — see below
    verify_mode,        // Full or Skip
    alpn_protocols,     // offered ALPN list
)
```

Two Fetches sharing the same fingerprint share one `Arc<ClientConfig>`. Cache entries become unreachable when the last referencing Fetch is dropped, and are reclaimed by Arc refcount on the next sweep.

**CRL fingerprint = source identity, not content.** `CrlSource::File(path)` hashes the path string; `CrlSource::Url(url)` hashes the URL string. The fetched CRL bytes are **not** part of the fingerprint. Consequence: when a CRL file is re-read from disk or a CRL URL returns fresh bytes, the fingerprint is unchanged, the cached `Arc<ClientConfig>` is unchanged, and the new CRL content is installed by mutating the rustls `CryptoProvider`'s CRL provider — new handshakes on the existing `ClientConfig` see the refreshed revocation list immediately.

Rationale: hashing CRL content would force a new `ClientConfig` on every CRL refresh, defeating the cache and producing connection-pool churn every few hours. The TLS `ClientConfig` identity stays stable across CRL updates; in-flight TLS connections keep serving (a revoked cert caught by a fresh CRL affects _new_ handshakes, not established ones — which is correct: in-flight sessions already completed identity verification at handshake time).

### mTLS on upstream

`client_cert: Some(Arc<CertifiedKey>)` presents a client cert to the upstream during handshake. Combined with the upstream's requirement for it on its side, this establishes mutual authentication.

### `CertifiedKey` is `Arc`-shared everywhere

Both sides of the TLS surface use `Arc<CertifiedKey>`:

- Listener side: `CertEntry.key: Arc<CertifiedKey>` (already defined above)
- Upstream side: `UpstreamTls.client_cert: Option<Arc<CertifiedKey>>`

`rustls::sign::CertifiedKey` is deliberately not `Clone` (it holds signing-key material), so `Arc` is the only reasonable sharing primitive. Populators construct one `Arc<CertifiedKey>` per loaded cert at refresh time; every rule referencing the same cert shares that Arc. The `TlsConfigFingerprint`'s `client_cert` field hashes the Arc's inner `(cert_der, key_id)` — two rules that independently load the same cert file produce the same fingerprint (and thus share one `Arc<ClientConfig>`), while a rotated cert gets a new Arc and a new fingerprint.

### CRL checking

When `crls` is non-empty, `rustls::WebPkiCrlProvider` validates the upstream cert against the provided CRL list. CRLs come from files (loaded at boot, optionally re-read on refresh) or URLs (fetched periodically from the CRL distribution point).

Two Fetches with different CRL source sets get separate pool slots (the source list participates in the fingerprint); two Fetches with identical CRL sources share one `ClientConfig` even as the underlying CRL bytes refresh (see fingerprint note above).

---

## Architected but deferred in MVP

These features have architectural positions defined above; MVP implementation order defers some:

- **OCSP stapling** — populator framework exists; `ManagedCertPopulator` fetches OCSP on cert issuance in its first release. `StaticCertPopulator` gains optional OCSP fetch later.
- **CRL checking** — `UpstreamTls.crls` defined; `WebPkiCrlProvider` integration and URL fetcher post-MVP.
- **Session ticket rotation** — `TicketKeyManager` designed; rotation task post-MVP. MVP uses rustls's default static ticket key per daemon.
- **TLS 1.3 0-RTT** — config flags and runtime check designed; rustls early-data wiring post-MVP. MVP ships with `enable_0rtt: false` hardcoded.
- **mTLS on listener** — `ClientAuth` enum and `ClientTrustStore` defined; MVP ships `ClientAuth::None` only; `Request` / `Require` post-MVP.
- **`ManagedCertPopulator` (integrated LazyCert)** — trait defined; MVP ships only `StaticCertPopulator`. ACME integration post-MVP (Stage 3) via `instant-acme`.

## ACME challenge modes

`ManagedCertPopulator` supports both RFC 8555 challenge modes:

### HTTP-01

For public-facing domains on port 80. `vaned` listens on `:80` and serves the `/well-known/acme-challenge/<token>` path via an internal synthetic responder during issuance; the response flows through the normal `HttpSynthesize` fetch path so it is visible in flow logs and subject to L1 security floor like any other traffic. No extra port bind.

**Local testing**: [Pebble](https://github.com/letsencrypt/pebble) — Let's Encrypt's official test ACME server — via the `testcontainers` crate. `vane-testutil::pebble()` spins up `letsencrypt/pebble` on a free port, points the `ManagedCertPopulator` at it (via a `directory_url` config override), runs the HTTP-01 dance end-to-end. Integration tests for HTTP-01 live in `tests/engine_acme_http01.rs`.

### DNS-01

For domains where port 80 is unreachable or for wildcard certs. Requires a DNS provider API to create `_acme-challenge.<domain>` TXT records. DNS-01 is gated behind per-provider Cargo features:

| Feature               | Default | Provider           |
| --------------------- | ------- | ------------------ |
| `acme-dns-cloudflare` | off     | Cloudflare DNS API |

Additional providers (Route53, DigitalOcean, etc.) land as new features post-MVP — each is a separate `#[cfg(feature = "...")]`-gated module implementing a `DnsProvider` trait (create / delete TXT record, wait for propagation).

**Local testing**: a mock DNS server via [`hickory-server`](https://crates.io/crates/hickory-server) + Pebble pointed at it as its resolver. `vane-testutil::mock_dns()` returns a handle that exposes an API matching the internal `DnsProvider` trait — tests assert the populator requested the right TXT record, the mock serves it, Pebble's challenge validation sees it, issuance completes. Integration tests in `tests/engine_acme_dns01.rs`, gated behind the provider feature they exercise.

Real Cloudflare testing is `#[ignore]`'d by default (requires a real zone + API token); CI runs it on-demand via an opt-in flag.

All of the above can be added without refactoring — interfaces and data structures are in place today.
