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

#### Populator lifecycle

Populators are FlowGraph-scoped — a fresh instance is constructed on every `FlowGraph::link`. Live TLS connections are unaffected (handshake-time cert is captured), but populator in-memory state does not survive a reload.

`StaticCertPopulator` is stateless: every reload re-reads cert files from disk; no persistence layer is needed.

`ManagedCertPopulator` is a **view over a daemon-scoped `ManagedCertRegistry`**: ACME accounts, the renewal scheduler, the pending-challenges table, and the persistent `AcmeStore` all live on the registry, which exists for the daemon's lifetime. The FlowGraph-scoped populator is constructed each reload and tells the registry which SNIs the new graph wants managed; it pulls cached cert state into a `CertStore` at `initial_store()` and `refresh()` time. ACME state is therefore unaffected by config reload, and rate-limit ceilings (Let's Encrypt's 5-duplicate-cert-per-domain-per-week, 50-cert-per-registered-domain-per-week) are not reachable through reload churn alone. Full design — registry shape, `AcmeStore` trait, configuration schema, challenge mechanics, renewal triggers, mgmt verbs — is in [`spec/acme.md`](../acme.md).

`refresh()` runs periodically (default: every 5 minutes). Each populator decides what is stale — near-expiry cert, expired OCSP response, ARI-suggested renewal window. If stale, return a new `CertStore` for `ArcSwap` to install.

### OCSP stapling

OCSP stapling is carried inline on the cert: `CertifiedKey.ocsp: Option<Vec<u8>>`. During handshake, rustls staples these bytes to the ServerHello automatically.

The populator is responsible for keeping OCSP fresh. OCSP responses typically validate for 4–7 days; refresh daily.

### Session ticket rotation

Session tickets let clients resume TLS sessions without a full handshake. The server encrypts session state with a key that must rotate periodically — a leaked key compromises all sessions encrypted with it.

`vane` uses [`rustls::TicketRotator`](https://docs.rs/rustls/latest/rustls/struct.TicketRotator.html) via the crypto backend's `Ticketer::new()` constructor (`rustls::crypto::aws_lc_rs::Ticketer::new()` or `rustls::crypto::ring::Ticketer::new()` per the active feature). The constructor returns an `Arc<dyn ProducesTickets>` whose internal `RwLock<{ current, previous, next_switch_time }>` provides the same current/previous semantics as the `TicketKeyManager` shape originally drafted here:

- new tickets are encrypted with `current`;
- `current` and `previous` both decrypt incoming tickets;
- once `next_switch_time` is reached, the next `encrypt` / `decrypt` call lazily demotes `current → previous` and generates a fresh `current` — no background task, no cancellation handling, no `ArcSwap` plumbing.

The crypto-backend constructors use the RFC 5077 "Recommended Ticket Construction" (AES-256-CBC + HMAC-SHA256, randomly generated keys per rotation), with a 6-hour rotation period and a 12-hour ticket lifetime (a ticket may be accepted up to twice the rotation period).

A daemon-wide `Arc<dyn ProducesTickets>` is installed at boot and shared by every listener's `ServerConfig.ticketer`. Configurable lifetime is post-MVP.

### TLS 1.3 0-RTT (early data)

0-RTT (TLS 1.3 early data, RFC 8446 §2.3) lets clients send application data in the first flight, saving one round-trip on resumption.

#### Replay risk — operator's responsibility

0-RTT data is **not replay-protected** — an attacker who captures a 0-RTT ClientHello can replay it. The client cannot tell, the server has no signal.

vane gates 0-RTT via a method check (idempotent only: GET / HEAD / OPTIONS) but **cannot guarantee** that the application's handler for those methods is actually idempotent. Common counter-examples:

- `GET /api/charge?amount=10` — encodes a side-effecting action in a GET.
- `GET /api/visit-counter` — increments per request.
- Any GET path that touches authn state (rate-limit counters, login attempt logs).

When operator opts a rule into 0-RTT, the operator is asserting that the rule's upstream handler is replay-safe. vane's compile-time gate (method check) is necessary but not sufficient; verifying handler idempotency end-to-end is on the operator.

Defense-in-depth recommendation: run rate-limit middleware **without** 0-RTT (so abusive replay is rate-bounded by the full-handshake path); reserve 0-RTT for static-asset or genuinely idempotent read-only endpoints.

#### Configuration

Two-level opt-in. Both fields are required (no implicit defaults; CLI / TUI emits `false` when 0-RTT is not in use):

```rust
pub struct ListenerTlsConfig {
    pub enable_zero_rtt: bool,
}

pub struct Rule {
    // ... other fields ...
    pub allow_zero_rtt: bool,   // only present on rules whose listener is TLS-terminating
}
```

| Field                          | Type | Required                                                |
| ------------------------------ | ---- | ------------------------------------------------------- |
| `listener.tls.enable_zero_rtt` | bool | yes, on every TLS-terminating listener                  |
| `rule.allow_zero_rtt`          | bool | yes, on every L7 rule whose listener is TLS-terminating |

`allow_zero_rtt` on an L4-only rule (no TLS termination on its listener) is a compile error.

#### Compile-time constraints

- `allow_zero_rtt: true` is only legal on rules whose match predicates include a method constraint restricted to idempotent methods: `GET`, `HEAD`, `OPTIONS`. Method check is structural — the rule must contain `{ "http.method": { "equals": "GET" } }` or the equivalent across `any_of` of those three. Rules with no method constraint, or with a non-idempotent method, fail compile if `allow_zero_rtt: true`.
- `allow_zero_rtt: true` on a rule whose listener has `enable_zero_rtt: false` is a compile error (the rule could never serve 0-RTT anyway, so the field is misleading).

#### Runtime flow

```
Client sends 0-RTT data
  ↓ rustls decrypts, exposes as "early data"
  ↓ vane parses Request, walks FlowGraph to matched rule
  ↓ request has a body?
      ├─ yes → rustls buffers early data; effectively downgrades to 1-RTT (handshake completes before body delivered)
      └─ no  → continue
  ↓ rule.allow_zero_rtt is false?
      ├─ yes → reject early data, rustls sends HRR, client retries with full handshake
      └─ no  → accept, proceed normally
```

#### Hardcoded limits

- **Early data size**: 16 KiB (rustls default `max_early_data_size`). Not exposed as a knob — 0-RTT exists to save one RTT, not to carry payload; raising the limit invites misuse. If a real workload needs more, revisit as a focused decision.
- **Body in 0-RTT**: requests with a body are always served via 1-RTT (early data is buffered until handshake completion, then released to the application as if it were 1-RTT). 0-RTT for HTTP request bodies is non-standard semantics; this rule keeps vane within HTTP/1.1 + HTTP/2 conventions.

### Client certificate verification (mTLS on listener)

Listener may request or require client certificates:

```rust
pub enum ClientAuth {
    None,                             // don't request client cert
    Request {                         // request, don't require; log if provided
        trust_store: Arc<ArcSwap<ClientTrustStore>>,
    },
    Require {                         // require; fail handshake if missing or invalid
        trust_store: Arc<ArcSwap<ClientTrustStore>>,
    },
}

pub struct ClientTrustStore {
    pub cas:  RootCertStore,
    pub crls: Vec<CrlSource>,         // see § CRL checking
}
```

mTLS is **per-listener**, not per-rule. The TLS handshake completes before rule routing, so a listener owns one `ClientAuth` configuration; per-rule authorization is expressed through predicates on the verified cert (see _Predicate fields_ below).

#### Configuration schema

```jsonc
"tls": {
  "sni":       "api.example.com",
  "cert_path": "/etc/vaned/certs/api.pem",
  "key_path":  "/etc/vaned/certs/api.key",
  "client_auth": {
    "mode": "require",
    "trust_store": {
      "ca_paths": ["/etc/vaned/ca/clients.pem"],
      "ca_dir":   "/etc/vaned/ca/clients.d/",
      "crls": [
        { "kind": "file", "path": "/etc/vaned/crls/clients.crl",       "fetch_failure": "tolerate" },
        { "kind": "url",  "url":  "https://crl.example.com/clients.crl", "fetch_failure": "tolerate" }
      ]
    }
  }
}
```

| Field                     | Type                                   | Required             | Notes                                                                  |
| ------------------------- | -------------------------------------- | -------------------- | ---------------------------------------------------------------------- |
| `client_auth.mode`        | `"none"` \| `"request"` \| `"require"` | yes                  | No implicit default. CLI/TUI emits the value the operator chose.       |
| `client_auth.trust_store` | object                                 | iff `mode != "none"` | Compile error if absent when `mode` is `request` or `require`.         |
| `trust_store.ca_paths`    | list\<string\>                         | no (one of two)      | Explicit PEM bundle paths; CAs concatenated.                           |
| `trust_store.ca_dir`      | string                                 | no (one of two)      | Directory; every `*.pem` inside is loaded and merged.                  |
| `trust_store.crls`        | list\<object\>                         | no                   | Empty list = no CRL revocation. See § CRL checking for `crls[]` shape. |

At least one of `ca_paths` or `ca_dir` must be present when `trust_store` is present; both may be present and are merged with de-duplication (CA dedup is by SubjectKeyIdentifier).

`trust_store` is reloaded on FlowGraph reload — the watcher (`notify`) sees changes to `ca_paths` files, `ca_dir` contents, and `crls` file paths through the existing config-watch pipeline. URL CRLs follow the fetch policy in § CRL checking.

There is **no `allowed_subject_cn` field** or any other allowlist knob on `trust_store`. Per-rule authorization is expressed via predicates so that the authorization decision is observable in `compile --dry-run`, in flow logs, and in the metric surface — instead of split between two mechanisms.

#### Request vs Require semantics

- `Require`: handshake aborts on missing or invalid client cert. Connections that reach rule routing are guaranteed to have a verified `peer_cert`.
- `Request`: handshake proceeds with or without a client cert. Connections may or may not have `peer_cert` populated.

Predicate fields under `tls.peer_cert.*` are populated only when a verified cert is present. The `tls.peer_cert.present: bool` predicate disambiguates the two cases — useful for the `Request` mode pattern of "if cert provided, gate by CN; else fall through to public route".

#### Predicate fields on `peer_cert`

The verified peer cert is exposed through a fixed set of paths (full grammar in [`18-predicate-schema.md`](18-predicate-schema.md)):

| Path                               | Type           | Use case                                             |
| ---------------------------------- | -------------- | ---------------------------------------------------- |
| `tls.peer_cert.present`            | `bool`         | Was a verified client cert presented?                |
| `tls.peer_cert.subject_cn`         | `String`       | Subject Common Name (legacy identity)                |
| `tls.peer_cert.san_dns`            | `Vec<String>`  | DNS-type Subject Alternative Names                   |
| `tls.peer_cert.fingerprint_sha256` | `String` (hex) | Full-cert pinning                                    |
| `tls.peer_cert.spki_sha256`        | `String` (hex) | Subject-public-key-info pinning (rotation-friendly)  |
| `tls.peer_cert.issuer_cn`          | `String`       | Federated trust — which internal CA signed this cert |
| `tls.peer_cert.serial`             | `String` (hex) | Audit / revocation correlation                       |

```json
{ "any_of": [
  { "tls.peer_cert.san_dns": { "contains": "svc-a.internal" } },
  { "tls.peer_cert.san_dns": { "contains": "svc-b.internal" } }
] }
{ "tls.peer_cert.fingerprint_sha256": { "equals": "ab12cd34..." } }
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

### Client cache: fingerprint and reuse

`rustls::ClientConfig` construction is expensive, and the H1/H2 upstream client built on top of it (`hyper_util::client::legacy::Client` over `hyper_rustls::HttpsConnector`) carries its own per-authority pool. Daemon caches the **entire client** behind a fingerprint so two Fetches with the same TLS posture share both the config and the pool:

```rust
daemon.client_cache: DashMap<ClientFingerprint, Arc<Client<HttpsConnector<HttpConnector>, Body>>>

ClientFingerprint = (version, Option<TlsConfigFingerprint>)   // tls = None on cleartext

TlsConfigFingerprint = hash(
    root_ca_source,     // System: constant tag; Bundle(path): path string
    client_cert,        // SHA256 of CertifiedKey's cert DER (Stage 2: always None; mTLS lands later)
    crl_sources,        // hash of CRL *sources* — see below; Stage 2: always empty
    verify_mode,        // Full or Skip
    alpn_protocols,     // offered ALPN list, derived from `version`
)
```

`version` participates because the connector wires ALPN via `enable_http1` / `enable_http2`, producing distinct `Client` instances per `UpstreamVersion` even when the TLS posture is identical. This is `07-l7.md` § _Pool fingerprint_'s `TcpFingerprint = (addr, version_slot, tls_hash)` minus `addr`: `hyper_util::Client` already keys its own pool by authority, so the cache only needs to split at the `(version, tls)` granularity.

Two Fetches sharing the same fingerprint share one `Arc<Client>`. The cache grows monotonically across reload cycles; entries are not actively swept in MVP — for the typical fingerprint count a daemon produces (a handful per ruleset) the bookkeeping is not worth the complexity. Forced removal lands post-MVP via a `pool.drain <fingerprint>` management verb (see `07-l7.md`).

**CRL fingerprint = source identity, not content.** `CrlSource::File(path)` hashes the path string; `CrlSource::Url(url)` hashes the URL string. The fetched CRL bytes are **not** part of the fingerprint. Consequence: when a CRL file is re-read from disk or a CRL URL returns fresh bytes, the fingerprint is unchanged, the cached `Arc<Client>` is unchanged, and the new CRL content is installed by mutating the rustls `CryptoProvider`'s CRL provider — new handshakes on the existing config see the refreshed revocation list immediately.

Rationale: hashing CRL content would force a new client on every CRL refresh, defeating the cache and producing connection-pool churn every few hours. The TLS config identity stays stable across CRL updates; in-flight TLS connections keep serving (a revoked cert caught by a fresh CRL affects _new_ handshakes, not established ones — which is correct: in-flight sessions already completed identity verification at handshake time).

### mTLS on upstream

`client_cert: Some(Arc<CertifiedKey>)` presents a client cert to the upstream during handshake. Combined with the upstream's requirement for it on its side, this establishes mutual authentication.

#### Configuration schema

```jsonc
"upstream": {
  "address": "backend.internal:443",
  "tls": {
    "verify": "full",
    "client_cert": {
      "cert_path": "/etc/vaned/upstream/client.pem",
      "key_path":  "/etc/vaned/upstream/client.key"
    }
  }
}
```

| Field                       | Type   | Required                         | Notes                             |
| --------------------------- | ------ | -------------------------------- | --------------------------------- |
| `tls.client_cert.cert_path` | string | yes (when `client_cert` present) | PEM file holding leaf + chain.    |
| `tls.client_cert.key_path`  | string | yes (when `client_cert` present) | PEM file holding the private key. |

`client_cert` itself is optional (omit it for one-way TLS). When present, both fields are required.

Cert/key files are read at FlowGraph link time. Rotation goes through the standard config-watch pipeline: `notify` sees the file change, debounces, triggers FlowGraph reload, the new `Arc<CertifiedKey>` lands in the new `UpstreamTls`. Live connections keep their handshake-time cert (TLS protocol does not permit mid-connection rotation; this is symmetric to the listener-side rotation rule above).

A missing or unreadable cert/key file at link time is a **rule-level compile error**, not a daemon-wide boot failure — other rules continue to compile.

### `CertifiedKey` is `Arc`-shared everywhere

Both sides of the TLS surface use `Arc<CertifiedKey>`:

- Listener side: `CertEntry.key: Arc<CertifiedKey>` (already defined above)
- Upstream side: `UpstreamTls.client_cert: Option<Arc<CertifiedKey>>`

`rustls::sign::CertifiedKey` is deliberately not `Clone` (it holds signing-key material), so `Arc` is the only reasonable sharing primitive. Populators construct one `Arc<CertifiedKey>` per loaded cert at refresh time; every rule referencing the same cert shares that Arc. The `TlsConfigFingerprint`'s `client_cert` field hashes the Arc's inner `(cert_der, key_id)` — two rules that independently load the same cert file produce the same fingerprint (and thus share one `Arc<ClientConfig>`), while a rotated cert gets a new Arc and a new fingerprint.

### CRL checking

CRLs are used in two places — listener-side mTLS (`ClientTrustStore.crls`, see § _Client certificate verification_) and upstream verification (`UpstreamTls.crls`). Both share the same source schema, fetch policy, and daemon-wide cache.

When `crls` is non-empty, `rustls::WebPkiCrlProvider` validates the relevant cert (peer client cert on the listener side; upstream cert on the upstream side) against the provided CRL list. CRLs come from files or URLs.

#### `crls[]` source schema

```jsonc
"crls": [
  { "kind": "file", "path": "/etc/vaned/crls/clients.crl",         "fetch_failure": "tolerate" },
  { "kind": "url",  "url":  "https://crl.example.com/clients.crl", "fetch_failure": "reject"   }
]
```

| Field           | Type                       | Required | Notes                                                                     |
| --------------- | -------------------------- | -------- | ------------------------------------------------------------------------- |
| `kind`          | `"file"` \| `"url"`        | yes      | Discriminates the variant.                                                |
| `path`          | string                     | iff file | Absolute filesystem path. File is re-read on FlowGraph reload.            |
| `url`           | string                     | iff url  | Absolute URL. Fetched per § _URL fetch cadence_.                          |
| `fetch_failure` | `"tolerate"` \| `"reject"` | yes      | What to do when the source becomes unavailable. See § _Failure handling_. |

#### URL fetch cadence

Adaptive based on the CRL's `nextUpdate` field:

- Parse the fetched CRL's `nextUpdate`. Schedule the next fetch for `nextUpdate − 1 hour`.
- If the CRL has no `nextUpdate`, fall back to a fixed 4-hour interval.
- The first fetch happens at FlowGraph link time (synchronous; rule compile blocks on it for at most 30 seconds before timing out).
- On fetch success, replace the cached bytes and reschedule against the new `nextUpdate`.

The schedule is held by a daemon-wide CRL fetcher, not per-config, per § _Daemon-wide CRL cache_.

#### Failure handling

A CRL source is considered "unavailable" when:

- A URL fetch fails (DNS, connection, HTTP error, parse error, signature verification failure).
- A file path cannot be read or parsed.
- The cached CRL is past its `nextUpdate` AND the most recent refetch attempt failed.

`fetch_failure` chooses behavior on unavailability:

- `"tolerate"` — keep using the last-known CRL (even if stale); if no CRL has ever been successfully loaded, behave as if the source were absent (no revocation check from this source). Log at WARN per transition (success → unavailable, unavailable → success); silent during sustained outage.
- `"reject"` — handshake validations against this source fail-closed. Log at ERROR per transition; new connections that would have validated against this CRL are rejected at handshake time.

`tolerate` matches the high-availability default that browsers and most production proxies adopt; `reject` is for high-security deployments where a stale CRL is unacceptable. The choice is per-source and explicit — there is no daemon-wide knob and no implicit default.

CRL Distribution Points encoded in the cert (`CRL Distribution Points` extension, RFC 5280 §4.2.1.13) are **not** auto-discovered. Operators configure CRL sources explicitly. This keeps network behavior predictable and prevents attacker-controlled CDP URLs from becoming a covert channel.

#### Daemon-wide CRL cache

The daemon holds a single `Arc<CrlCache>` keyed by source identity (`CrlSourceId = (kind, path-or-url-string)`). Every `ClientTrustStore` and `UpstreamTls` configuring the same source shares one cached entry, fetched once per refresh interval, served to every consumer.

This composes with the `TlsConfigFingerprint` rule "CRL fingerprint = source identity, not content" (see § _Client cache: fingerprint and reuse_): `Arc<ClientConfig>` cache entries stay stable across CRL refresh cycles, while the underlying CRL bytes update through the rustls `CryptoProvider`'s CRL provider in place. New handshakes on existing client configs see fresh revocation data immediately.

Two configs with different CRL source sets get separate pool slots (the source list participates in the TLS fingerprint); two configs with identical CRL sources share one `ClientConfig` even as the bytes refresh.

#### CRL and OCSP coexistence

CRL and OCSP stapling are independent revocation channels and may both be configured. rustls runs both checks; either channel returning "revoked" rejects the handshake (logical OR over revocation verdicts). This is the conventional defense-in-depth posture; vane does not synthesize a precedence between the two.

OCSP staples are per-handshake and freshness-bounded by the OCSP response's `nextUpdate`; CRL is daemon-cached and freshness-bounded by `fetch_failure` policy. Operators wanting strict revocation correctness configure both for their critical chains.

---

## Architected but deferred in MVP

These features have architectural positions defined above; MVP implementation order defers some:

- **OCSP stapling** — populator framework exists; `ManagedCertPopulator` fetches OCSP on cert issuance in its first release. `StaticCertPopulator` gains optional OCSP fetch later.
- **CRL checking** — Stage 3. Source schema, adaptive fetch cadence, per-source `fetch_failure`, daemon-wide CRL cache, and CRL/OCSP coexistence are all specified above in § _CRL checking_. Implementation lands with S3-11.
- **Configurable session-ticket lifetime** — MVP uses the crypto-backend `Ticketer::new()` default (12-hour ticket lifetime, 6-hour rotation period). A configurable lifetime knob is post-MVP.
- **TLS 1.3 0-RTT** — full design locked in § _TLS 1.3 0-RTT (early data)_ above: `enable_zero_rtt` / `allow_zero_rtt` field schema, idempotent-method gate, body-downgrade rule, 16 KiB hardcoded early data size. Implementation lands with S3-13.
- **mTLS on listener** — Stage 3. `ClientAuth` enum, `ClientTrustStore`, the `client_auth` config schema, the seven `tls.peer_cert.*` predicate paths, and the Request-vs-Require semantics are specified above in § _Client certificate verification_. Implementation lands with S3-12.
- **`ManagedCertPopulator` (integrated LazyCert)** — Stage 3. Full design — daemon-scoped `ManagedCertRegistry`, `AcmeStore` persistence, `tls.managed` config schema, HTTP-01 / DNS-01 mechanics, ARI-driven renewal, `force_renew` mgmt verb — is locked in [`spec/acme.md`](../acme.md). Built on [`instant-acme`](https://crates.io/crates/instant-acme).

## ACME challenge modes

`ManagedCertPopulator` supports HTTP-01 and DNS-01 from RFC 8555. TLS-ALPN-01 is intentionally not implemented — see [`spec/acme.md`](../acme.md) § _Challenge: TLS-ALPN-01 — not implemented_ for the rationale.

- **HTTP-01** — when any rule declares `tls.managed.challenge == "http-01"`, the compiler injects a high-priority `/.well-known/acme-challenge/` route into every plaintext `:80` listener; if no plaintext `:80` listener exists in the operator's config, `vaned` auto-binds one for the challenge path only. No extra burden on operator routing config. Detail in [`spec/acme.md`](../acme.md) § _Challenge: HTTP-01_.
- **DNS-01** — required for wildcard SANs and for domains behind unreachable `:80`. Each provider is a separate `#[cfg(feature = "...")]`-gated module implementing the `DnsProvider` trait. Stage 3 ships `acme-dns-cloudflare` (off by default). Trait shape and per-provider config schemas in [`spec/acme.md`](../acme.md) § _Challenge: DNS-01_.

Local testing uses [Pebble](https://github.com/letsencrypt/pebble) via `testcontainers` for the ACME server side, and [`hickory-server`](https://crates.io/crates/hickory-server) for the mock DNS side. Integration test layout in [`spec/acme.md`](../acme.md) § _Testing_.
