# ACME

Authoritative design for `vane`'s automatic certificate issuance and renewal via the `ManagedCertPopulator`. Implements RFC 8555 (ACME core) plus RFC 9773 (Auto Renewal Information / ARI) over the [`instant-acme`](https://crates.io/crates/instant-acme) client.

Listener TLS plumbing (cert resolver, populator trait, OCSP, rotation, session tickets) lives in [`architecture/08-tls.md`](architecture/08-tls.md). This file specifies _only_ the ACME-specific parts.

## Architecture

`ManagedCertRegistry` is **daemon-scoped**, not FlowGraph-scoped. A single registry lives for the daemon's lifetime and owns all ACME state:

- ACME accounts, keyed by directory URL.
- `instant-acme` clients.
- The pending-challenges table (consulted by the HTTP-01 responder and during the DNS-01 dance).
- The renewal scheduler.
- An `Arc<dyn AcmeStore>` for persistence.

`ManagedCertPopulator` (which implements `CertPopulator` per `08-tls.md`) is FlowGraph-scoped and is a _view_ over the registry: at construction it tells the registry which SNIs the new FlowGraph wants managed; at `refresh()` it pulls cached cert state from the registry into a new `CertStore` for `ArcSwap` installation.

This separation isolates ACME state lifetime (daemon-scoped, survives reload) from cert delivery (FlowGraph-scoped, rebuilt per reload). Reload churn does not cause new order requests; rate-limit ceilings are not exposed to operator config-cycling.

```rust
// Daemon-level: constructed once at boot, lives until shutdown.
pub struct ManagedCertRegistry {
    store:     Arc<dyn AcmeStore>,
    accounts:  DashMap<DirectoryUrlHash, Arc<AcmeAccount>>,
    pending:   DashMap<(Sni, ChallengeToken), PendingChallenge>,
    certs:     DashMap<Sni, Arc<RegisteredCert>>,
    schedule:  Arc<RenewalScheduler>,
}

// FlowGraph-scoped: constructed per FlowGraph::link, dropped on next swap.
pub struct ManagedCertPopulator {
    registry: Arc<ManagedCertRegistry>,
    watched:  Vec<ManagedCertSpec>,    // sourced from rule-side `tls.managed`
}

#[async_trait]
impl CertPopulator for ManagedCertPopulator {
    async fn initial_store(&self) -> Result<CertStore> {
        // Tell the registry to ensure all `watched` certs are tracked,
        // then pull whatever certs are currently valid into a CertStore.
        // Missing ones come through future refresh() calls as the
        // registry obtains them.
        ...
    }

    async fn refresh(&self, current: &CertStore) -> Result<Option<CertStore>> {
        // Compare registry's current cert state for `watched` SNIs
        // against `current`; return a new CertStore if any changed.
        ...
    }
}
```

## `AcmeStore` trait

Persistence abstraction. Default impl: `FsAcmeStore` (filesystem, see § _Storage layout_). Other backends (object store, secrets manager) implement the same trait.

```rust
#[async_trait]
pub trait AcmeStore: Send + Sync {
    async fn load_account(&self, directory_url: &str)
        -> Result<Option<AcmeAccount>>;
    async fn save_account(&self, directory_url: &str, account: &AcmeAccount)
        -> Result<()>;

    async fn load_cert(&self, sni: &str)
        -> Result<Option<StoredCert>>;
    async fn save_cert(&self, sni: &str, cert: &StoredCert)
        -> Result<()>;
    async fn list_cert_snis(&self) -> Result<Vec<String>>;

    async fn with_lock<F, T>(&self, scope: &str, f: F) -> Result<T>
    where
        F: for<'a> FnOnce(&'a dyn AcmeStore)
            -> futures::future::BoxFuture<'a, Result<T>> + Send;
}

pub struct AcmeAccount {
    pub directory_url:  String,
    pub key_jwk:        serde_json::Value,
    pub kid:            String,
    pub contacts:       Vec<String>,
    pub agreed_tos_at:  SystemTime,
}

pub struct StoredCert {
    pub leaf_pem:           String,
    pub chain_pem:          String,
    pub key_pem:            String,
    pub not_after:          SystemTime,
    pub ari_replacement_id: Option<String>,
    pub last_renew_at:      SystemTime,
}
```

`with_lock` provides advisory locking scoped to a string (e.g. account directory URL or cert SNI). The default fs impl uses `flock(2)` on a `.lock` file beside the target.

## Storage layout (default `FsAcmeStore`)

```
/var/lib/vaned/acme/
  accounts/
    <directory_url_hash>/
      account.json        # AcmeAccount, mode 0600
      .lock
  certs/
    <sni>/
      cert.pem            # leaf + intermediate chain
      key.pem             # private key, mode 0600
      meta.json           # not_after, ari_replacement_id, last_renew_at
      .lock
```

- `accounts/` is `0700`; private keys are `0600`; other files `0644`.
- `<directory_url_hash>` is `sha256(directory_url)[..16]` (hex). Keeps multi-CA support open without committing to multi-CA today.
- `<sni>` is the SNI string lowercased, with `*` replaced by `_wild_` for filesystem safety.

## Configuration schema

Rule-side config under `tls.managed`:

```jsonc
{
	"tls": {
		"sni": "api.example.com",
		"managed": {
			"directory_url": "https://acme-v02.api.letsencrypt.org/directory",
			"contact": ["mailto:ops@example.com"],
			"agree_tos": true,
			"challenge": "http-01",
			"key_type": "ecdsa-p256",
			"renew_before": "30d",
			"san": ["api.example.com", "www.api.example.com"]
		}
	}
}
```

Every field is required. The JSON is generated by `vane`'s CLI / TUI, not hand-written by operators, so verbosity is free and the absence of any field is a compile error rather than an implicit default. The only exception is `account_key_path` (BYO mode opt-in) and `dns_provider` (only meaningful when `challenge == "dns-01"`).

| Field              | Type                           | Required               | Notes                                                                           |
| ------------------ | ------------------------------ | ---------------------- | ------------------------------------------------------------------------------- |
| `directory_url`    | string                         | yes                    | Required to avoid silently hitting LE prod in dev.                              |
| `contact`          | list\<string\>                 | yes                    | One or more contact URIs (`mailto:` is the common case).                        |
| `agree_tos`        | bool                           | yes                    | Must be `true`. Compile error if absent or `false`.                             |
| `challenge`        | `"http-01"` \| `"dns-01"`      | yes                    | No auto-detect. Explicit picks avoid surprises.                                 |
| `dns_provider`     | object (provider-specific)     | iff `challenge=dns-01` | Schema is per-impl, Cargo-feature-gated.                                        |
| `account_key_path` | string                         | no                     | BYO account key. Absence means auto-create+persist via `AcmeStore`.             |
| `key_type`         | `"ecdsa-p256"` \| `"rsa-2048"` | yes                    | Subject key algorithm. CLI/TUI emits `"ecdsa-p256"` by default.                 |
| `renew_before`     | duration                       | yes                    | Same duration grammar as `rate_limit.window`. CLI/TUI emits `"30d"` by default. |
| `san`              | list\<string\>                 | yes                    | Subject Alternative Names. Must include `tls.sni`. CLI/TUI emits `[tls.sni]`.   |

### Compile-time checks

- Any required field missing → error pointing at the missing field.
- `agree_tos != true` → error `"tls.managed.agree_tos must be true"`.
- `challenge == "dns-01"` without `dns_provider` → error.
- `san` contains a wildcard label (`*.example.com`) but `challenge != "dns-01"` → error.
- `san` does not contain `tls.sni` → error (the issued cert must match the SNI clients send).
- HTTP-01 challenge declared but no plaintext `:80` listener exists in the config → warn at compile (auto-bind will be attempted at runtime); see § _Challenge: HTTP-01_.

## Account key strategy

- Default: auto-create on first use of a `directory_url`. The generated key is persisted via `AcmeStore::save_account` and reused on subsequent boots.
- BYO via `account_key_path`: load PEM at boot; do not persist via `AcmeStore`. Useful for migration and CI.
- An account is keyed by `directory_url`: a vaned with two distinct directory URLs holds two distinct accounts. Rare but supported.
- ToS acceptance is recorded in `AcmeAccount.agreed_tos_at` at registration time. CA-side ToS-version bumps that require re-acceptance surface as a registration error. The operator must update `agree_tos` (still `true`) and reload to re-accept; this is treated as an active reaffirmation.

## SAN and wildcard policy

- Each `tls.managed` block produces exactly one cert covering all SANs in `san`. Multi-rule deduplication is the operator's responsibility — two rules declaring identical `san[]` issue two distinct certs.
- Wildcard SANs (`*.example.com`) require `challenge == "dns-01"`. This is an ACME protocol constraint, not a vane policy.
- The cert's CommonName is set to `san[0]`; subsequent SANs are SAN-list only (no CN duplication).

## Challenge: HTTP-01

ACME validators issue `GET http://<domain>/.well-known/acme-challenge/<token>` over plaintext HTTP/1.1 to the IP `<domain>` resolves to. `vaned` serves these requests via one of two mechanisms, picked at compile time based on whether the operator's config already includes a plaintext `:80` listener.

### Case 1 — operator has a plaintext `:80` listener

The compiler **injects a high-priority synthetic route** into every plaintext-listener whose port is 80. The injected node has a strict static predicate; non-ACME traffic is unaffected.

Static predicate (compile-time):

```
listener.port      == 80
listener.kind       in { http, auto-plaintext }
http.version       == "HTTP/1.1"
http.method        == "GET"
http.uri.path     starts-with "/.well-known/acme-challenge/"
```

Fetch: `HttpSynthesizeFetch::AcmeChallenge { registry: Arc<ManagedCertRegistry> }`. Priority: above all operator rules on this listener.

The fetch handler does the dynamic part:

1. Extract the token from the path tail.
2. Look up `(Host header, token)` in `ManagedCertRegistry::pending`.
3. If found: respond `200 OK` with `Content-Type: application/octet-stream` and the key-authorization body.
4. If not found: respond `404 Not Found`. The `/.well-known/acme-challenge/` namespace is reserved by `vane` whenever ACME is in use; falling through to operator rules would surface ACME plumbing as if it were ordinary 404s.

The injected route is visible in `vane compile --dry-run`, annotated as `[acme-injected]` in the dry-run output and in flow logs at runtime. Because of its priority, a `redirect_https` rule on the same listener does not swallow ACME validation traffic.

### Case 2 — no plaintext `:80` listener exists

`vaned` attempts to bind a synthetic plaintext listener on `:80` (dual-stack `0.0.0.0:80` + `[::]:80`, following the rules of S1-14). The synthetic listener serves only the ACME challenge route from Case 1; anything else returns `404`.

- On successful bind: a `WARN`-level log announces the auto-binding:

  ```
  acme: auto-bound :80 plaintext listener for HTTP-01 challenges;
  configure an explicit :80 listener to suppress this notice
  ```

- On bind failure (port in use, `EACCES` on a privileged port without `CAP_NET_BIND_SERVICE`): an `ERROR`-level log fires with the OS error. Affected ACME issuances will fail their HTTP-01 challenge; the registry surfaces the failure in `get_certs` (status `failed`, `last_error` populated). `vaned` does not abort boot on this failure — other functionality continues; the operator sees the issue in logs and mgmt verbs.

### Conflict and edge cases

- `:80` listener exists but is TLS-only (HTTPS on port 80, unusual): treated as "no plaintext :80"; auto-bind is attempted and will fail (port collision); ACME issuance fails until reconfigured. A compile-time warning is emitted.
- Multiple plaintext `:80` listeners on different bind addresses (e.g. `0.0.0.0:80` and `192.168.1.5:80`): the injected route lands in every one, since the CA validator's chosen path through the network is non-deterministic.
- A plaintext `:80` listener with operator-defined rules whose match would overlap `/.well-known/acme-challenge/`: the injected node has higher priority, so vane's responder wins. `compile --dry-run` annotates the operator's overlapping rule as `[shadowed-by-acme]`.

## Challenge: DNS-01

For domains where port 80 is unreachable (private networks, CGNAT, IPv6-only) or for wildcard certs, validation goes through DNS TXT records on `_acme-challenge.<domain>`.

### `DnsProvider` trait

```rust
#[async_trait]
pub trait DnsProvider: Send + Sync + 'static {
    /// Create a TXT record. Returns an opaque handle for later deletion.
    async fn set_txt(&self, fqdn: &str, value: &str)
        -> Result<TxtRecordHandle, DnsError>;

    /// Block until `value` is observable for `fqdn` from external resolvers.
    /// Implementations choose their propagation check.
    async fn wait_propagated(&self, fqdn: &str, value: &str, timeout: Duration)
        -> Result<(), DnsError>;

    /// Best-effort cleanup. Called even when validation fails.
    async fn delete_txt(&self, handle: TxtRecordHandle)
        -> Result<(), DnsError>;
}

pub struct TxtRecordHandle(/* provider-specific opaque token */);

pub enum DnsError {
    Auth(String),
    Network(String),
    NotPropagated,
    NotFound,
    Internal(String),
}
```

Each provider impl defines its own `Config: serde::Deserialize` matching the rule-side `dns_provider` JSON object.

### `wait_propagated` semantics

Implementations should observe propagation via two channels and return when either fires:

1. **Provider authoritative status** — if the provider's API exposes a "TXT applied to authoritative servers" signal (e.g. Cloudflare's `success` flag).
2. **Public resolver query** — query a small set of public recursive resolvers (`8.8.8.8`, `1.1.1.1`) for the TXT record and confirm `value` is observed.

The CA validator queries DNS from its own resolvers; passing both checks above is a high-confidence proxy. `timeout` defaults to 60 seconds at the registry level and is configurable per `dns_provider`.

### Available providers

Each provider is gated behind a Cargo feature; only the feature-on impl(s) ship in any given binary.

| Feature      | Default | Provider           |
| ------------ | ------- | ------------------ |
| `cloudflare` | off     | Cloudflare DNS API |

The Cloudflare config schema:

```jsonc
"dns_provider": {
  "kind":          "cloudflare",
  "api_token_env": "CF_API_TOKEN",  // env var holding the API token
  "zone_id":       "abc123..."      // optional; auto-detected if omitted
}
```

Tokens are read from environment variables, never from the JSON config — the JSON is reloadable, the env var is set at startup; this matches the `09-config.md` `.env`-vs-config split.

Additional providers (Route 53, DigitalOcean, …) land as separate features post-MVP. Each is a `#[cfg(feature = "...")]`-gated module implementing `DnsProvider`.

## Challenge: TLS-ALPN-01 — not implemented

`vane` does not implement TLS-ALPN-01. Its only unique scenario (`:80` unreachable AND DNS provider unavailable) is rare enough that the protocol-level disruption cost (special-case ALPN dispatch on `:443`, listener-side cert-resolver branching) is not worth supporting. Operators in that constraint use DNS-01.

## Renewal triggers

Three independent triggers may cause a renewal attempt:

### Periodic timer (primary)

`ManagedCertRegistry`'s scheduler ticks every 5 minutes (matches `08-tls.md`'s `refresh()` cadence). On each tick, for every registered cert:

- If `now + renew_before >= not_after` — start renewal.
- If an ARI window is set and `now ∈ window` — start renewal.
- Otherwise — skip.

### ARI (RFC 9773)

If a cert came from a CA that supports ARI, the cert's metadata includes a `renewalInfo` URL. The scheduler queries this URL after each successful issuance and records the CA-suggested `suggestedWindow.start..end`. Renewal is triggered when wall-clock falls inside the window, regardless of `renew_before`.

ARI lets the CA spread renewal load and signal forced rotation (e.g. CA-side incident requiring early rotation). Honoring ARI is recommended by Let's Encrypt and is the 2025+ industry default.

### `force_renew` mgmt verb

Operators can trigger immediate renewal:

```
vane force_renew --sni api.example.com
```

This bypasses the periodic timer and ARI window; the cert is queued for immediate renewal. Useful for key-compromise rotation. Verb shape:

| Field   | Type                                       |
| ------- | ------------------------------------------ |
| Verb    | `force_renew`                              |
| Args    | `{ "sni": string }`                        |
| Returns | `{ queued: bool, current_status: string }` |

## Rate-limit and failure handling

Let's Encrypt rate limits (representative; other CAs vary):

- 50 certificates per registered domain per week.
- 5 duplicate certificates (identical SAN set) per week.
- 300 new orders per account per 3 hours.
- 10 account registrations per IP per 3 hours.

`ManagedCertRegistry` does not pre-empt these limits — caching last-known limits client-side is fragile and not the goal. Instead, on a CA error of class `urn:ietf:params:acme:error:rateLimited`, the registry:

1. Records `last_error`, `next_attempt_at = now + backoff`, status = `limited`.
2. Backoff: exponential, base 30 minutes, factor 2, capped at 24 hours; resets to base on first success.
3. Logs at `WARN` on each backoff transition; subsequent retries during the same backoff window are silent.

Other failure classes (network timeout, DNS provider error, validation timeout): same backoff schedule, status = `failed`.

`get_certs` exposes the per-cert state for operator visibility.

## mgmt verbs

| Verb          | Purpose                                                                  |
| ------------- | ------------------------------------------------------------------------ |
| `get_certs`   | List all certs (managed + static) with status, SAN, expiry, error state. |
| `force_renew` | Trigger immediate renewal for a single SNI. Bypasses timer + ARI window. |

`get_certs` response shape:

```jsonc
{
	"certs": [
		{
			"sni": "api.example.com",
			"source": "managed", // or "static"
			"san": ["api.example.com"],
			"not_after": "2026-08-04T12:34:56Z",
			"issued_at": "2026-05-06T12:34:56Z",
			"status": "valid", // valid | renewing | failed | limited
			"last_attempt_at": "2026-05-01T03:00:00Z",
			"last_error": null,
			"next_attempt_at": null,
			"ari_window": { "start": "...", "end": "..." }
		}
	]
}
```

These verbs follow the snake_case convention from `architecture/10-management.md`.

## Testing

### HTTP-01

[Pebble](https://github.com/letsencrypt/pebble) — Let's Encrypt's official test ACME server — via the `testcontainers` crate. `vane-testutil::pebble()` spawns Pebble on a free port; integration tests in `tests/engine_acme_http01.rs` point `directory_url` at it and exercise both code paths:

- One test against a vaned configured with an explicit `:80` listener (verifies the inject path).
- One test against a vaned configured without `:80` (verifies the auto-bind path).

### DNS-01

A mock DNS server via [`hickory-server`](https://crates.io/crates/hickory-server). `vane-testutil::mock_dns()` returns an in-process `DnsProvider` impl that records `set_txt` / `delete_txt` calls and serves the TXT through a `hickory-server` instance Pebble is configured to use as its resolver. Integration tests in `tests/engine_acme_dns01.rs`, gated behind the provider feature being exercised.

Real Cloudflare testing is `#[ignore]`'d by default (requires a real zone and API token); CI runs it on-demand via an opt-in flag.
