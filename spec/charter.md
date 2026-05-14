# Charter

Vane runs a production website behind one entry point. That is the entire user story.

## In scope

- L4 forward (TCP, UDP).
- HTTP any-bridge: H1.1 / H2 / H3 client × upstream, all nine combinations.
- WebSocket over H1.1 only.
- Upstream transports: TCP, Unix socket, CGI.
- TLS termination on L4→L7 upgrade, SNI peek without decrypt, upstream re-encryption.
- WASM plugins as the only extension mechanism.
- JSON config, multi-file merge, compile-then-swap hot reload.
- Management: Unix socket (default-on) + HTTP-over-TCP (default-on, loopback unless opted public).

## Out of scope (permanent)

- WebSocket over H2 (RFC 8441) or H3.
- Layer 5 / Layer 6 abstractions.
- Subprocess or FFI plugins.
- Byte-level TCP ↔ UDP bridging. Cross-transport bridging happens at the application layer (e.g. H3 ↔ H1.1).
- Non-HTTP application protocols (SMTP, SSH, MySQL). L4 forward handles these opaquely.
- Web UI for management. CLI + TUI only.
- Multi-tenant cloud operation.
- Windows. `vaned` assumes Unix signals, Unix sockets, `SO_REUSEADDR/PORT`, fork+exec, `CAP_NET_BIND_SERVICE` / systemd socket activation.
- `native-tls`, `openssl`, `boring`, `hyper-tls`. TLS is [`rustls`](https://crates.io/crates/rustls) only — the `aws-lc-rs` ↔ `ring` choice is rustls-internal. `rustls-native-certs` (pure Rust despite the name) is fine.

## Target user

A single developer or small ops team running one or a few servers.

## Design principles

Every downstream decision resolves against these:

1. **Pay as you go.** A connection pays only for the layers it reaches. Rules inspect → compile derives hooks → runtime executes the minimum.
2. **Config is IR; runtime is the compiled artifact.** JSON compiles to an immutable `FlowGraph`. The runtime walks the graph; it does not re-evaluate config.
3. **Declarative over procedural.** Users write match predicates. Hooks are a compile output.
4. **Graph validity at load time.** Every path reaches a Terminator. The runtime never handles "no matching rule".
5. **Zero-copy where the abstraction allows.** `Bytes` passthrough for bodies. TLS / HPACK / QPACK internal copies are out of vane's scope; vane promises no vane-introduced copy from ingress to egress.
6. **No `dyn Any`, no string-keyed KV.** Typed `http::Extensions` for protocol-specific fields.

## Non-goals that look like goals

- **Cloudflare / Nginx parity.** Vane borrows ideas (rule-based config, WASM isolation). It does not compete on scale or directive-cascade ergonomics.
- **Protocol research.** No custom wire formats. Ride on `http`, `hyper`, `h3`, `quinn`.

## Release profile

`[profile.release]` in the workspace `Cargo.toml` uses `opt-level = "z"` plus `lto = true`, `codegen-units = 1`, `strip = true`, `panic = "abort"`. This is a deliberate trade — binary size and cold-start footprint over peak throughput. The target user runs **one** vaned per host and is bandwidth- or fan-out-bound long before they are CPU-bound; an extra few percent of inlining headroom is not visible at one-server-per-team scale, and the smaller binary materially helps container image size, system-package payload, and TLB / I-cache pressure on the cold path.

Do not switch to `opt-level = "3"` without first putting numbers on a workload that is demonstrably CPU-bound under vane's hot path (the executor + per-request flow walk). If a real benchmark shows a meaningful win, update this section in the same commit.
