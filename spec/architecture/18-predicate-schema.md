# Predicate Schema

The wire format and serde design for rule `match` predicates, plus the grammar of field paths and the compatibility matrix between operators and value types. Pairs with `02-flow.md`'s `PredicateInst` (runtime form) and `04-middleware.md`'s `L7RequestMiddleware.inspects()` (feature discovery).

## Shape overview

A predicate is one of four forms, serialized as JSON:

```jsonc
// Combinator: OR over children.
{ "any_of": [ <predicate>, <predicate>, ... ] }

// Combinator: AND over children.
{ "all_of": [ <predicate>, <predicate>, ... ] }

// Combinator: negation.
{ "not": <predicate> }

// Check: a single-key object whose key is a field path and whose value
// is an externally-tagged operator enum.
{ "<field-path>": { "<operator>": <value> } }
```

Top-level `match` on a rule is an **implicit AND** — an array of predicates that must all hold:

```jsonc
{
	"rule": "web-api",
	"listen": [":443"],
	"match": [
		{ "tls.sni": { "equals": "api.example.com" } },
		{ "http.header.host": { "equals": "api.example.com" } },
		{
			"any_of": [
				{ "http.method": { "equals": "GET" } },
				{ "http.method": { "equals": "HEAD" } }
			]
		}
	],
	"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" }
}
```

## Rust type definitions

```rust
pub enum Predicate {
    AnyOf(AnyOfP),
    AllOf(AllOfP),
    Not(NotP),
    Check(CheckMap),
}

pub struct AnyOfP { pub any_of: Vec<Predicate> }
pub struct AllOfP { pub all_of: Vec<Predicate> }
pub struct NotP   { pub not:    Box<Predicate> }

/// A single-key map: the key is a field path, the value is the operator.
/// Deserializes from `{ "<field-path>": { "<operator>": <value> } }`.
pub struct CheckMap {
    pub path: FieldPath,
    pub op:   Operator,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Equals(Value),
    NotEquals(Value),
    Contains(Value),
    NotContains(Value),
    Prefix(Value),
    Suffix(Value),
    Matches(String),           // regex pattern, validated at compile
    In(Vec<Value>),
    NotIn(Vec<Value>),
    Gt(i64), Gte(i64), Lt(i64), Lte(i64),
    Cidr(String),              // CIDR notation, validated at compile
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    // Bytes: accepted as base64-encoded string; detected by field-path type at compile.
}
```

## Serde derivation

The shape is fully derivable with stock serde — no custom `Deserialize` impl:

```rust
#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum Predicate {
    AnyOf(AnyOfP),
    AllOf(AllOfP),
    Not(NotP),
    Check(CheckMap),
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnyOfP { pub any_of: Vec<Predicate> }

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllOfP { pub all_of: Vec<Predicate> }

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotP { pub not: Box<Predicate> }
```

### How disambiguation works

`#[serde(untagged)]` tries variants in declaration order. For an input object:

1. **`AnyOf`** — matches if the object has a single field named `any_of` whose value is an array of predicates. `deny_unknown_fields` ensures it does **not** match an object that has `any_of` plus other keys.
2. **`AllOf`** — same pattern with `all_of`.
3. **`Not`** — same pattern with `not`.
4. **`Check`** — the fallback. Any single-key object with a non-combinator key falls here. `CheckMap` has a custom one-line `Deserialize` that reads the map's only key as `FieldPath` and the value as `Operator`.

Only `CheckMap`'s `Deserialize` is custom — and it's ~15 lines. The combinator variants are pure derive.

### Why this doesn't need reserved-word policy

The audit initially asked "what if a user has a field named `any_of`?" Looking at the authoritative field-path grammar below: field paths are drawn from a **fixed closed set** (`transport`, `remote.*`, `peek`, `tls.*`, `http.method`, `http.uri.*`, `http.header.<name>`, `http.body`). None of these top-level paths matches `any_of` / `not` / `all_of`. Only nested paths (e.g., `http.header.any_of` — an HTTP header literally named "any_of") are legal in principle, but those are **multi-segment dotted paths**, not bare single-token keys. The untagged deserializer distinguishes them cleanly: `{"any_of": ...}` always means the combinator; `{"http.header.any_of": ...}` always means the Check form.

### Combinator semantics

`any_of` is the OR combinator: matches if any child matches. Empty `any_of: []` is vacuously **false** (matches nothing).

`all_of` is the AND combinator: matches if every child matches. Empty `all_of: []` is vacuously **true** (matches everything). The top-level `match: [A, B, C]` array remains the user-facing implicit-AND shorthand; `all_of` is the explicit form, useful inside `any_of` (where the top-level array is unavailable) — e.g., `any_of: [ all_of: [upgrade==websocket, path.prefix=/ws], all_of: [upgrade==websocket, path.prefix=/api/stream] ]`.

`not` is negation: swaps the on_match and on_miss edges of its child at lower time, so it never adds a node — it costs zero runtime cycles.

### Cross-level combinators rejected

A combinator (`any_of` / `all_of` / `not`) whose Check leaves mix levels (e.g., `tls.sni` + `http.method`) is rejected by `lower` with a pointed error. Reason: a single Check node has one Phase placement, and the executor's `PredicateView` enum is variant-per-phase — there is no representation for "evaluate this leaf at L4Peeked, that one at L7Request". Users compose cross-level logic at the rule layer (one rule per phase) rather than inside a single combinator.

## Field path grammar

```ebnf
field-path  = segment ("." segment)*
segment     = [a-z_] [a-z0-9_-]*         ; lowercase letters, digits, underscore, hyphen
```

Lowercase-only by rule. Mixed-case field paths are a parse error. The parser's error message suggests the lowercased form:

```
error: field path must be lowercase
       at rules/30-api.json:14: "http.header.Host"
       did you mean `http.header.host`?
```

HTTP header names are case-insensitive (RFC 9110 §5.1) but vane's canonical internal form is always lowercase — both in the predicate grammar and in the `hyper::HeaderMap` that `http.header.<name>` reads from. The suggestion is cheap (lowercase the offending segment) and eliminates the commonest new-user friction.

### Authoritative field paths

| Path                               | Value type            | Source                                                            |
| ---------------------------------- | --------------------- | ----------------------------------------------------------------- |
| `transport`                        | enum `"tcp" \| "udp"` | `ConnContext.transport`                                           |
| `remote.ip`                        | `IpAddr`              | `ConnContext.remote.ip()`                                         |
| `remote.port`                      | `u16`                 | `ConnContext.remote.port()`                                       |
| `local.ip`                         | `IpAddr`              | `ConnContext.local.ip()`                                          |
| `local.port`                       | `u16`                 | `ConnContext.local.port()`                                        |
| `peek`                             | `Bytes`               | `PeekResult.buffer` (L4 peek phase)                               |
| `tls.sni`                          | `String`              | `ConnContext.tls.sni`                                             |
| `tls.alpn`                         | `Bytes`               | `ConnContext.tls.alpn`                                            |
| `tls.version`                      | enum `TlsVersion`     | `ConnContext.tls.version`                                         |
| `tls.peer_cert.present`            | `bool`                | `true` iff a verified peer cert is attached to this connection    |
| `tls.peer_cert.subject_cn`         | `String`              | Subject Common Name; empty string when `present == false`         |
| `tls.peer_cert.san_dns`            | `Vec<String>`         | DNS-type Subject Alternative Names; empty when `present == false` |
| `tls.peer_cert.fingerprint_sha256` | `String` (hex, lower) | SHA-256 of the full DER-encoded leaf cert                         |
| `tls.peer_cert.spki_sha256`        | `String` (hex, lower) | SHA-256 of the cert's SubjectPublicKeyInfo (rotation-stable)      |
| `tls.peer_cert.issuer_cn`          | `String`              | Issuer Common Name                                                |
| `tls.peer_cert.serial`             | `String` (hex, lower) | Cert serial number, big-endian, no leading-zero stripping         |
| `http.method`                      | enum `Method`         | `Request.method()`                                                |
| `http.uri.path`                    | `String`              | `Request.uri().path()`                                            |
| `http.uri.query`                   | `String`              | `Request.uri().query().unwrap_or("")`                             |
| `http.header.<name>`               | `String`              | first value of `Request.headers()[name]`                          |
| `http.body`                        | `Bytes`               | **request-side** buffered body; triggers request-track LazyBuffer |

`<name>` in `http.header.<name>` is a header name: lowercased, hyphens allowed. Duplicate-valued headers (e.g., `Cookie`) expose the first value; users needing "any of the values" combine with `any_of`.

`http.body` reads the **request** body, which is a sole side today — response-body inspection is deliberately not in the MVP grammar. When added, it will take a distinct path (`http.response.body`) so that the two LazyBuffer tracks (see `02-flow.md`) stay independently analyzable.

### Field path → inspection level

The compiler's `analyze` pass categorizes each path into one of three inspection levels for rule sorting:

| Path prefix                                    | Inspection level                                                    |
| ---------------------------------------------- | ------------------------------------------------------------------- |
| `transport` / `remote.*` / `local.*`           | `L4-only`                                                           |
| `peek` / `tls.*`                               | `L4-peek` (falls under L4 for sorting, but needs ClientHello parse) |
| `http.method` / `http.uri.*` / `http.header.*` | `L7-header`                                                         |
| `http.body`                                    | `L7-body`                                                           |

`L4-only < L4-peek < L7-header < L7-body` for the "deeper first" sort.

## Operator × value type compatibility

| Operator                    | Str | Bytes | Int | IpAddr | enum | Bool | `Vec<Str>` |
| --------------------------- | :-: | :---: | :-: | :----: | :--: | :--: | :--------: |
| `equals` / `not_equals`     | yes |  yes  | yes |  yes   | yes  | yes  |     —      |
| `contains` / `not_contains` | yes |  yes  |  —  |   —    |  —   |  —   |    yes     |
| `prefix` / `suffix`         | yes |  yes  |  —  |   —    |  —   |  —   |     —      |
| `matches`                   | yes |   —   |  —  |   —    |  —   |  —   |     —      |
| `in` / `not_in`             | yes |  yes  | yes |  yes   | yes  |  —   |     —      |
| `gt` / `gte` / `lt` / `lte` |  —  |   —   | yes |   —    |  —   |  —   |     —      |
| `cidr`                      |  —  |   —   |  —  |  yes   |  —   |  —   |     —      |

`Vec<Str>` (currently only `tls.peer_cert.san_dns`) supports `contains` / `not_contains` with a single-string operand — semantics: "the list contains / does not contain this exact element". For "any of `[a, b, c]` is in the list" composition, use the top-level `any_of` combinator wrapping multiple `contains` Checks.

`Bool` (currently only `tls.peer_cert.present`) supports `equals` / `not_equals` against a JSON boolean literal.

Compile-time type check: on a `{ "http.body": { "gt": 100 } }`, the compiler sees `http.body: Bytes` and `gt: numeric-only` → rejects with:

```
error: operator `gt` cannot apply to field `http.body` (expected numeric, got Bytes)
       rules/30-api.json:14
```

Similarly, `{ "http.uri.path": { "cidr": "10.0.0.0/8" } }` fails: `cidr` is IP-only.

## Value JSON encoding

```jsonc
// String-valued fields
{ "equals": "api.example.com" }

// Bytes-valued fields: base64
{ "contains": "aGVsbG8=" }

// Integer-valued fields
{ "gt": 1024 }

// Lists
{ "in": ["foo", "bar", "baz"] }

// CIDR (field type = IpAddr, operator = cidr)
{ "cidr": "10.0.0.0/8" }

// Regex (field type = String, operator = matches)
{ "matches": "^/api/v\\d+/users" }
```

The `Value` serde enum's `untagged` representation auto-infers from JSON type (string → `Str`, number → `Int`, etc.). Bytes fields accept base64-encoded strings; the compiler decodes at `lower` time based on the field's known type.

## Regex specifics

Matched at compile time using `fancy-regex`. Safeguards applied at **compile time** (before the pattern reaches the runtime):

- **Pattern source size limit** — regex pattern string ≤ 4 KiB. `fancy-regex` does not expose the compiled NFA size directly, so we bound the input instead of the compiled artifact; patterns that need more than 4 KiB of source are almost certainly adversarial or misdesigned. Rejected at compile with the rule file + line.
- **Backtrack step limit** — every `Regex` is constructed with `set_backtrack_limit(1_000_000)` so malformed input cannot consume unbounded CPU at runtime. Patterns not using lookaround / backreferences are delegated to the `regex` crate internally and have no backtracking.

Compile errors on bad patterns name both the rule file and the offending operator:

```
error: invalid regex in `matches` operator on field `http.uri.path`
       rules/30-api.json:14
       caused by: unknown escape sequence at position 5
```

## CIDR specifics

```jsonc
{ "remote.ip": { "cidr": "10.0.0.0/8" } }
{ "remote.ip": { "cidr": "2001:db8::/32" } }
```

Parsed via `ipnet::IpNet::from_str`. Mixing IPv4 and IPv6 CIDRs in `in`/`not_in` is allowed; a single `cidr` operator matches only the specified family.

## Compilation: `Predicate` → `PredicateInst`

The `lower` pass (see `02-flow.md`) transforms each parsed `Predicate` into `PredicateInst`:

| Operator                                                             | Compilation step                                                                   |
| -------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `Equals`, `NotEquals`, `Contains`, `NotContains`, `Prefix`, `Suffix` | Value coerced to field's native type (string → `Arc<str>`, base64 → `Bytes`, etc.) |
| `Matches`                                                            | Pattern compiled to `fancy_regex::Regex` with backtrack limit                      |
| `In`, `NotIn`                                                        | Each element coerced; stored as `Vec<CompiledValue>`                               |
| `Gt`/`Gte`/`Lt`/`Lte`                                                | Stored as `i64`                                                                    |
| `Cidr`                                                               | Parsed to `ipnet::IpNet`                                                           |

All failures produce compile errors with rule name + file + line pointers from `RawRule::source` (see `14-presets.md`).

## Runtime: `PredicateInst::test`

Test is evaluated by the executor at every `Node::Check`. It receives a `PredicateView` — a phase-aware window onto only the state that is legal to read in the current phase:

```rust
pub enum PredicateView<'a> {
    L4 {
        conn: &'a Arc<ConnContext>,
        peek: Option<&'a [u8]>,     // Some iff phase == L4Peeked
    },
    L7Req {
        conn: &'a Arc<ConnContext>,
        req:  &'a Request,          // body is Body::Static iff a request-side LazyBuffer trigger preceded this check
    },
    // L7Resp variant is reserved for future response-side field paths; none exist today.
}

impl PredicateInst {
    pub fn test(&self, view: &PredicateView<'_>) -> bool { /* dispatch on self.path + self.op */ }
}
```

Why a phase-typed view instead of universal field access: the executor has only the state that the phase owns. In `Phase::L4Raw` there is no `Request` to read `http.method` from; in `Phase::L7Response` the `Request` has already been consumed by `L7Fetch::fetch` (see `05-terminator.md`). Encoding the phase into the `PredicateView` enum makes it a compile error for `test` implementations to reach for state that does not exist.

The `lower` pass participates in this invariant: when it emits a `Check` node on path `P`, it picks the `PredicateView` variant matching `P`'s phase at that point. Field paths whose inspection level does not fit that phase are rejected at compile time with the same error style as other validate failures (`rules/30-api.json:14`).

Body access inside `test`: for `http.body`, the reader is `view.req.body().as_static().expect("lazy-buffer invariant")`. The `.expect` is sound because the analyze pass marks the Check node's incoming edge with `collect_body_before = Some(BodySide::Request)` on the first reader, and executor collects before entering the node (see `02-flow.md`).

## Hash-consing

`PredicateInst` is `Hash + Eq` so the compiler deduplicates structurally-identical predicates across rules. Two rules both writing `{ "tls.sni": { "equals": "api.example.com" } }` share one `PredicateId`.

- `fancy_regex::Regex` — equality by pattern source string (canonicalized via `as_str()`)
- `ipnet::IpNet` — equality by canonical form (network addr + prefix length)
- `CompiledValue::Str(Arc<str>)` — equality by string content (not by Arc identity)

This means editor-level whitespace differences in rule files never defeat dedup, but intentionally-different regex source strings (e.g., `a|b` vs `b|a`) are treated as distinct — the compiler does not rewrite regexes for structural equivalence.

### Dedup is cross-phase, runtime is not

`PredicateInst` hash-consing is **phase-agnostic**: two rules checking `tls.sni == "api.com"`, one on an `L4Peeked` path and one on an `L7Request` path, share a single `PredicateId`. This is sound because a field path's _value domain_ (what it reads from ctx) does not change between phases that both admit the read — the lookup code is the same whether called from a peek-phase Check or a request-phase Check.

`Node::Check` sharing is a separate question. A Check node's identity is `(predicate, on_match, on_miss, collect_body_before)`. Two Check nodes with the same predicate but different `on_match` targets are distinct nodes; they happen to share a `PredicateId`. A single Check node that is reachable from two different phase contexts (rare, but legal) is covered by the validator's `(NodeId, Phase)` seen-set (see `02-flow.md`).

What hash-consing does **not** do: it does not skip runtime `test()` calls. Each `execute()` invocation that walks through a `Node::Check` runs `test()` once on that walk. Two HTTP/2 streams on the same connection hitting the same Check therefore call `test()` twice — this is the correct semantics (streams are independent requests), not a redundancy. Per-connection memoization of predicate results for fields that are provably connection-invariant (`tls.sni`, `remote.ip`) is a post-MVP optimization; MVP just calls `test()` each time.

## Extensibility rules

The authoritative field path list grows **only** by source change in `vane-core`'s path resolver. Adding a new path:

1. Add the path → value type row in the table above.
2. Wire the path reader into `PredicateInst::test`'s dispatch.
3. Update `analyze`'s inspection-level table.
4. If the new path touches a resource that isn't always present (e.g., TLS-only), `analyze` must recognize this and emit a compile error when a rule uses the path on a non-TLS-capable listener.

WASM plugins **do not** contribute field paths — plugin-driven predicates go through the `Wasm` middleware variant and use the plugin's own `inspects: list<string>` metadata (see `11-wasm.md`), not this field-path grammar.
