# vane-engine: TLS

Source: [`crates/engine/src/tls/`](../../crates/engine/src/tls/), [`listener.rs`](../../crates/engine/src/listener.rs).

TLS termination, cert resolution, mTLS, OCSP, CRL, session tickets, 0-RTT. ACME-driven cert population is in [`engine-acme.md`](engine-acme.md).

## Library policy

[`rustls`](https://crates.io/crates/rustls) only. The following are non-dependencies by policy — a PR introducing any of them is rejected:

- `native-tls` — wraps the host OS's TLS library (SChannel / Secure Transport / OpenSSL). A competitor.
- `openssl`, `openssl-sys`, `boring`, `boring-sys` — FFI to OpenSSL / BoringSSL.
- `hyper-tls` — hyper connector built on `native-tls`. Vane uses `hyper-rustls`.

Confusingly-named ally that is in scope: [`rustls-native-certs`](https://crates.io/crates/rustls-native-certs) — pure Rust despite the name. Loads OS root-CA bundle (via `security-framework` on macOS, `openssl-probe` filesystem lookup on Linux — the probe is filesystem, not an OpenSSL dep) and hands roots to rustls. If the crate name starts with `rustls-` or is a rustls-flavored feature on another crate, it is an ally; if it starts with `native-tls` / `openssl` / `boring` / `hyper-tls`, banned.

When pulling a crate that offers multiple TLS backends (`reqwest`, `hyper`, `hickory-resolver`), the `Cargo.toml` entry must carry `default-features = false` and explicitly select a rustls-flavored feature.

A `script/check-no-openssl.sh` CI step asserts `cargo tree --workspace` contains zero `openssl-sys` to catch regressions mechanically.

The `aws-lc-rs` ↔ `ring` choice in [`daemon.md` § _Crypto provider_](daemon.md#crypto-provider) is rustls-internal and unrelated to this policy.

## Two orthogonal dimensions

Client ↔ vaned and vaned ↔ upstream are independent. They compose into a 6-cell matrix, all combinations supported. No architecture-level "禁止" policy — operators pick their posture, and `vane compile` plus external lints can enforce organisational policies on top.

```
                         Upstream HTTP     Upstream HTTPS Full     Upstream HTTPS Skip
Listener :80 (HTTP)       plaintext         start TLS to origin     start TLS, no verify
Listener :443 (HTTPS)     client-TLS→plain  end-to-end TLS verified end-to-end TLS no-verify
```

## Listener-side TLS

### SNI peek (L4, no decrypt)

Built-in middleware reads the ClientHello from the peek buffer and populates `ctx.tls.sni` and `ctx.tls.alpn` without decrypting. Uses `rustls::server::Acceptor`: feed bytes via `read_tls`, call `accept()` for an `Accepted` exposing `client_hello() -> ClientHello<'_>`. No handshake yet — on the L4 routing path we abort here (drop the `Accepted`); on the TLS termination path we continue via `Accepted::into_connection(config)`. Same `Acceptor` serves both roles. Source: `crates/engine/src/middleware/sni_peek.rs`.

SNI is ASCII-lowercased at every ingress. The invariant "`ctx.tls.sni` is lowercase" holds system-wide:

- The peek middleware lowercases parsed `server_name`.
- `VaneCertResolver::resolve` lowercases `client_hello.server_name()` before lookup.
- `CertStore::by_sni` keys are stored lowercase at populator-time.
- The predicate compiler rejects `tls.sni` literals containing uppercase.

Non-ASCII hostnames follow IDNA: clients send punycode (`xn--`); vane does not attempt U-label → A-label conversion. Cert files via `StaticCertPopulator` must list hostnames in punycode form.

### Termination flow (L4 → L7 upgrade)

```
TCP stream
  ↓ peek ClientHello (L4, no decrypt)
  ↓ SNI-based cert lookup    → rustls::ServerConfig
  ↓ rustls handshake         → plaintext AsyncRead + AsyncWrite; ctx.tls populated
  ↓ ALPN dispatch            → hyper (H1/H2) or h3 (H3); ctx.http_version populated
  ↓ parse                    → Request
```

Source: `crates/engine/src/listener.rs::run_tls`, `crates/engine/src/upgrade.rs`.

### Version, cipher, ALPN

- TLS 1.3 preferred, 1.2 accepted, 1.0 / 1.1 / SSL all rejected. New-era engine; no legacy baggage.
- Cipher suites: rustls defaults. No custom tuning.
- ALPN per `ListenerKind`: `Http` → `h2`, `http/1.1`; QUIC listener → `h3`; `Auto` → all applicable, peer picks.

### Cert resolver

`VaneCertResolver` implements `rustls::server::ResolvesServerCert`. The `CertStore` is `ArcSwap`-managed; the resolver does explicit lookup-then-fallback (`store.by_sni.get(sni)` then `store.default`). We do not delegate to `rustls::server::ResolvesServerCertUsingSni` because that resolver returns `None` (handshake failure) on unmatched SNI with no fallback hook. Source: `crates/engine/src/tls/resolver.rs`, `crates/engine/src/tls/cert_store.rs`.

```rust
pub struct CertStore {
    by_sni:  HashMap<String, Arc<CertEntry>>,
    default: Option<Arc<CertEntry>>,    // optional no-SNI fallback
}

pub struct CertEntry {
    pub key:              Arc<CertifiedKey>,
    pub not_after:        SystemTime,
    pub ocsp_next_update: Option<Instant>,
}
```

SNI keys come from the rule layer's explicit `tls.sni` field, not parsed from the cert's SAN/CN. Parsing certs at compile time would couple the config layer to an x509 dependency and silently bind cert content changes to routing changes (a re-issued cert with a different SAN would silently rewrite the SNI map). Operators name the SNI they route on; the cert's contents must agree, but the routing key is the explicit field.

A rule whose `tls` has no `sni` field becomes the listener's `default` cert (one default per listener, enforced at lower).

Rotation is `ArcSwap` replacement of the inner `CertStore`. Live TLS connections keep their handshake-time cert; only new handshakes see the new cert. TLS protocol does not permit mid-connection cert change.

| Situation           | Behavior                                      |
| ------------------- | --------------------------------------------- |
| Client sends no SNI | Use `default_cert` if configured; else reject |
| SNI not in store    | Same                                          |
| No cert resolved    | TLS handshake fails; TCP closes               |

Default: reject. Opt-in fallback via `default_cert` in `config.json`. Silent mismatch (presenting a cert for the wrong domain) is worse than an explicit TLS error.

### Cert populators

```rust
pub trait CertPopulator: Send + Sync {
    async fn initial_store(&self) -> Result<CertStore>;
    async fn refresh(&self, current: &CertStore) -> Result<Option<CertStore>>;
}
```

Multiple populators can populate the same store keyed by SNI. Source: `crates/engine/src/tls/populator.rs`.

- `StaticCertPopulator` — loads cert/key files from configured paths. Optional OCSP response file, or optional OCSP fetch from cert's AIA URL on refresh. Stateless; every reload re-reads from disk. Source: `crates/engine/src/tls/static_populator.rs`.
- `ManagedCertPopulator` — a view over a daemon-scoped `ManagedCertRegistry`. ACME / Let's Encrypt automatic issuance and renewal via [`instant-acme`](https://crates.io/crates/instant-acme). See [`engine-acme.md`](engine-acme.md).

`refresh()` runs every 5 minutes. Each populator decides what is stale (near-expiry, expired OCSP, ARI-suggested window). If stale, return a new `CertStore`.

### OCSP stapling

OCSP carried inline on the cert: `CertifiedKey.ocsp: Option<Vec<u8>>`. During handshake, rustls staples to ServerHello automatically. Source: `crates/engine/src/tls/ocsp.rs`.

OCSP responder URLs in production CAs (Let's Encrypt, DigiCert, Sectigo, …) all use plaintext `http://`. Responses are independently signed by the CA's OCSP responder cert (RFC 6960 §4.2.2.1), so transport adds nothing the response signature does not already provide. HTTPS responders are exceptionally rare in deployment.

Vane commits to HTTP-only OCSP fetching. An HTTPS responder URL surfaces as `OcspError::HttpsNotSupported` and the cert ships without a staple; operators in that situation use `ocsp_path` to deliver a pre-fetched response on disk.

Refresh cadence: typically validates 4–7 days; refresh daily.

### Session tickets

`rustls::TicketRotator` via the active crypto backend's `Ticketer::new()`. Returns `Arc<dyn ProducesTickets>` whose internal state provides current/previous semantics:

- New tickets encrypt with `current`.
- Both `current` and `previous` decrypt incoming.
- Once `next_switch_time` reached, the next encrypt/decrypt lazily demotes `current → previous` and generates fresh `current`. No background task, no cancellation handling, no `ArcSwap` plumbing.

Defaults from RFC 5077 "Recommended Ticket Construction": AES-256-CBC + HMAC-SHA256, randomly generated keys per rotation. 6-hour rotation period, 12-hour ticket lifetime (ticket may be accepted up to twice the rotation period).

A daemon-wide `Arc<dyn ProducesTickets>` is installed at boot and shared by every listener's `ServerConfig.ticketer` — except listeners with `enable_zero_rtt: true` (see § _0-RTT_).

Source: `crates/engine/src/tls/ticketer.rs`.

### TLS 1.3 0-RTT (early data)

0-RTT (RFC 8446 §2.3) lets clients send data in the first flight, saving one round-trip on resumption.

#### Scope: TLS-over-TCP listeners only

H1.1 / H2. The H3 listener path is deferred — `quinn::RecvStream::is_0rtt()` exists but `h3-quinn` ≤ 0.0.10 does not expose it through `h3::server::RequestStream`, and connection-level proxies (tracking when `quinn::ZeroRttAccepted` resolves) admit a TOCTOU between stream-accept and handshake-complete that can leak a true 0-RTT request past an `allow_zero_rtt: false` rule. That trades security-positive false-positive for security-negative false-negative; we don't ship the negative side.

H3 0-RTT will be picked up when `h3-quinn` (or a successor) publishes a stable per-stream 0-RTT signal. Until then, the H3 listener always negotiates full 1-RTT regardless of `enable_zero_rtt`. Operators who configure `enable_zero_rtt: true` on a TLS+H3 listener get TCP-side 0-RTT and full-handshake H3.

```rust
// TODO(0rtt-h3): pick up H3 listener-side 0-RTT when h3-quinn surfaces a
// stable per-stream signal. Until then, H3 negotiates full 1-RTT regardless
// of enable_zero_rtt. See spec/crates/engine-tls.md § _Scope_.
```

Upstream-side 0-RTT (vane → upstream) is also out of scope — see [`engine.md` § _Upstream pools_](engine.md#upstream-pools).

#### Replay risk — operator's responsibility

0-RTT data is not replay-protected. Vane gates 0-RTT via a method check (idempotent only: GET / HEAD / OPTIONS) but cannot guarantee the application's handler for those methods is actually idempotent. Common counter-examples:

- `GET /api/charge?amount=10` — encodes a side-effecting action in a GET.
- `GET /api/visit-counter` — increments per request.
- Any GET path that touches authn state.

When the operator opts a rule into 0-RTT, the operator asserts the upstream handler is replay-safe. Vane's compile-time gate is necessary but not sufficient; verifying handler idempotency end-to-end is on the operator.

Defense-in-depth recommendation: run rate-limit middleware without 0-RTT (so abusive replay is rate-bounded by the full-handshake path); reserve 0-RTT for static-asset or genuinely idempotent read-only endpoints.

#### Configuration

Two-level opt-in, both fields required (no implicit defaults; CLI/TUI emits `false` when 0-RTT is not in use):

```rust
pub struct ListenerTlsConfig { pub enable_zero_rtt: bool }
pub struct Rule { pub allow_zero_rtt: bool }   // only on rules whose listener is TLS-terminating
```

#### Compile-time constraints

- `allow_zero_rtt: true` is only legal on rules whose match predicates include a method constraint restricted to GET / HEAD / OPTIONS.
- `allow_zero_rtt: true` on a rule whose listener has `enable_zero_rtt: false` is a compile error.
- `allow_zero_rtt` on an L4-only rule (no TLS termination) is a compile error.

#### Hardcoded limits

- Early data size: 16 KiB (rustls default `max_early_data_size`). Not exposed as a knob — 0-RTT exists to save one RTT, not to carry payload.
- Body in 0-RTT: requests with a body always serve via 1-RTT (early data buffered until handshake completion). 0-RTT for HTTP request bodies is non-standard semantics.

#### Ticketer interaction

rustls 0.23's TLS 1.3 server state machine refuses to accept 0-RTT when `ServerConfig.ticketer.enabled() == true`. Stateless rotating tickets (`TicketRotator` produces) cannot detect reuse — the load-bearing replay-attack mitigation; mixing the two would silently make 0-RTT unsafe.

Vane honors per listener:

- `enable_zero_rtt: false` (default): listener takes the daemon-wide rotating ticketer. Sessions resume across reload (encrypted ticket survives ServerConfig rebuilds). High-throughput non-0-RTT posture.
- `enable_zero_rtt: true`: listener's `ticketer` left at rustls default (`NeverProducesTickets`); `session_storage` defaults to per-`ServerConfig` `ServerSessionMemoryCache::new(256)`. Sessions resume only within a graph generation — a reload rebuilds storage and the first connection after reload pays full 1-RTT. Trade-off for replay-safe 0-RTT.

```rust
// TODO(0rtt-cross-reload): a daemon-wide Arc<dyn StoresServerSessions> would
// recover cross-reload session survival for 0-RTT listeners. Operators who
// need 0-RTT typically tolerate the brief post-reload warmup; revisit if
// profiling shows it matters.
```

### Client certificate verification (mTLS on listener)

Listener may request or require client certs. mTLS is per-listener — TLS handshake completes before rule routing — so a listener owns one `ClientAuth`; per-rule authorization is expressed via predicates on the verified cert.

```rust
pub enum ClientAuth {
    None,
    Request   { trust_store: Arc<ArcSwap<ClientTrustStore>> },
    Require   { trust_store: Arc<ArcSwap<ClientTrustStore>> },
}

pub struct ClientTrustStore {
    pub cas:  RootCertStore,
    pub crls: Vec<CrlSource>,
}
```

Source: `crates/engine/src/tls/client_trust.rs`.

Configuration shape, predicate fields on `peer_cert.*`, request vs require semantics, no-`allowed_subject_cn`-knob rationale: `crates/core/src/rule.rs` (config schema) and `crates/core/src/predicate.rs` (the seven `tls.peer_cert.*` paths).

`ClientTrustStore` rotates via `ArcSwap` symmetric to `CertStore`. Reloads on FlowGraph reload — the watcher (`notify`) sees changes through the existing config-watch pipeline.

There is no `allowed_subject_cn` field. Per-rule authorization is via predicates so the decision is observable in `compile <DIR>`, in flow logs, and in metrics — instead of split between two mechanisms.

## Upstream-side TLS

```rust
pub struct UpstreamTls {
    pub root_ca:     RootCaSource,                           // System (rustls-native-certs) | Bundle(PathBuf)
    pub client_cert: Option<Arc<CertifiedKey>>,              // mTLS to upstream
    pub crls:        Vec<CrlSource>,
    pub verify_mode: VerifyMode,                             // default Full
    pub alpn:        Option<Vec<String>>,                    // default: derived from HttpUpstream.version
    pub sni:         Option<String>,                         // override; default: derived from addr hostname
}

pub enum VerifyMode {
    Full,    // chain + hostname + CRL (if provided) + OCSP (if stapled)
    Skip,    // TLS handshake still runs; traffic still encrypted; identity not verified
}
```

`Skip` does not mean "no encryption". The TLS handshake still runs; traffic is still encrypted; only cert identity is skipped. Connection is still protected against passive eavesdropping, just not against active MITM.

### Client cache

`rustls::ClientConfig` construction is expensive, and the H1/H2 upstream client built on top (`hyper_util::client::legacy::Client` over `hyper_rustls::HttpsConnector`) carries its own per-authority pool. Daemon caches the entire client behind a fingerprint:

```rust
daemon.client_cache: DashMap<ClientFingerprint, Arc<Client<HttpsConnector<HttpConnector>, Body>>>

ClientFingerprint = (version, Option<TlsConfigFingerprint>)   // tls = None on cleartext

TlsConfigFingerprint = hash(
    root_ca_source,     // System: constant tag; Bundle(path): path string
    client_cert,        // SHA-256 of CertifiedKey's cert DER
    crl_sources,        // hash of CRL *sources* — see CRL fingerprint rule
    verify_mode,        // Full or Skip
    alpn_protocols,     // offered ALPN list, derived from `version`
)
```

`version` participates because the connector wires ALPN via `enable_http1` / `enable_http2` — distinct `Client` instances per `UpstreamVersion` even when TLS posture is identical.

Two fetches sharing the fingerprint share one `Arc<Client>`. The cache grows monotonically across reload cycles; no active sweep — for typical fingerprint counts (handful per ruleset), bookkeeping is not worth the complexity. Forced removal via `pool.drain <fingerprint>` mgmt verb.

CRL fingerprint = source identity, not content. Refresh updates rustls's CRL provider in place (see § _CRL_).

Source: `crates/engine/src/fetch/client_cache.rs`.

### `CertifiedKey` is `Arc`-shared

`rustls::sign::CertifiedKey` is deliberately not `Clone` (holds signing key material), so `Arc` is the only reasonable sharing primitive. Populators construct one `Arc<CertifiedKey>` per loaded cert at refresh time; rules referencing the same cert share the Arc. The fingerprint hashes the Arc's inner `(cert_der, key_id)` — two rules independently loading the same file produce the same fingerprint and share one `Arc<ClientConfig>`; a rotated cert gets a new Arc and a new fingerprint.

## CRL

Used in two places — listener-side mTLS (`ClientTrustStore.crls`) and upstream verification (`UpstreamTls.crls`). Both share source schema, fetch policy, and daemon-wide cache. Source: `crates/engine/src/tls/crl_cache.rs`, `crates/engine/src/tls/refreshable_crl_verifier.rs`.

When non-empty, `rustls::WebPkiCrlProvider` validates against the CRL list. Sources are files or URLs.

```jsonc
"crls": [
  { "kind": "file", "path": "/etc/vaned/crls/clients.crl",         "fetch_failure": "tolerate" },
  { "kind": "url",  "url":  "https://crl.example.com/clients.crl", "fetch_failure": "reject"   }
]
```

Schema, semantics of `fetch_failure` (tolerate/reject), full URL fetch cadence (adaptive based on `nextUpdate`, fall back to 4-hour interval): `crates/engine/src/tls/crl_cache.rs`.

CRL Distribution Points encoded in the cert (RFC 5280 §4.2.1.13) are not auto-discovered. Operators configure CRL sources explicitly. Keeps network behavior predictable and prevents attacker-controlled CDP URLs from becoming a covert channel.

Daemon-wide cache keyed by `CrlSourceId = (kind, path-or-url-string)`. Every `ClientTrustStore` and `UpstreamTls` configuring the same source shares one cached entry, fetched once per refresh interval, served to every consumer.

Two configs with different CRL source sets get separate pool slots (source list participates in the TLS fingerprint); two configs with identical sources share one `ClientConfig` even as bytes refresh.

CRL and OCSP coexist as independent revocation channels. rustls runs both; either returning "revoked" rejects the handshake (logical OR over revocation verdicts). Conventional defense-in-depth posture; vane does not synthesize a precedence between the two.
