# Charter

Vane is a network proxy daemon focused on **one user story**: running a production website behind one entry point.

## In scope (permanent)

- **L4 forwarding**: TCP and UDP byte-level forwarding between ports.
- **HTTP any-bridge**: HTTP/1.1, HTTP/2, HTTP/3 arbitrarily bridged client ↔ upstream. All nine version pairings supported.
- **WebSocket**: HTTP/1.1 upgrade only.
- **Upstream transports**: TCP, Unix socket, CGI.
- **TLS**: termination (on L4→L7 upgrade), SNI peek (L4 routing without decrypt), upstream re-encryption.
- **WASM plugins**: the only extension mechanism.
- **Config format**: JSON, multi-file merge, compile-then-swap hot reload.
- **Management**: Unix socket (default-on) + HTTP-over-TCP (opt-in remote).

## Permanently out of scope

These are not "later." They are explicit non-goals.

- **WebSocket over HTTP/2 (RFC 8441) or HTTP/3**: never.
- **Layer 5 or Layer 6 abstractions**: the model is strictly L4 or L7.
- **Subprocess or FFI plugins**: WASM is the only extension boundary.
- **Byte-level TCP ↔ UDP bridging**: semantically undefined. Cross-transport bridging exists only at the application protocol layer (e.g., HTTP/3 ↔ HTTP/1.1, handled by the HTTP any-bridge).
- **Non-HTTP application protocols**: no SMTP, SSH, MySQL, or similar as proxied protocols. L4 forward handles these opaquely when they happen to use TCP/UDP.
- **Web UI for management**: CLI + TUI only.
- **Multi-tenant cloud operation**: no tenant isolation beyond what WASM sandboxing provides for plugins.

## Deferred (may enter scope post-MVP)

- WASM instance-pool auto-scaling (MVP uses user-configured fixed pool sizes).
- Dedicated management-API verbs for standalone listener add / remove (MVP handles listener-set changes via config reload diff — see `01-topology.md` — but does not expose `listener.add` / `listener.remove` verbs).
- Automatic certificate acquisition (MVP accepts cert files; the pure-Rust LazyCert port-over from v1 lands after MVP).
- Upstream health checks and circuit breaking.
- Metrics/tracing exporters beyond structured logs.

## Target user

A single developer or small ops team running one or a few servers. Not a cloud vendor. Not a large enterprise ops desk.

## MVP slice (proposed)

Proposal, not final. Intended as the first concrete milestone after architecture sign-off.

- L4 TCP forward.
- HTTP/1.1 reverse proxy (client side and upstream side).
- Unix socket management, no remote HTTP management.
- JSON merge-compile-swap config.
- Four preset rule shapes: `port_forward`, `reverse_proxy`, `static_site`, `redirect_https`. WebSocket proxying is a `reverse_proxy` flag, not a separate preset.
- Built-in middleware: SNI peek, HTTP protocol detect, path match, header rewrite.
- No WASM (internal middleware only).
- No TUI (CLI only).
- No HTTP/2, HTTP/3, upstream TLS, or WebSocket.
- Listeners locked at boot.

Anything not in this list ships post-MVP.

## Design principles

Every downstream decision resolves against these. When a document in this directory appears to conflict with one of them, the document is wrong.

1. **Pay as you go.** A connection pays only for the layers it actually reaches. Rules inspect → compilation derives hooks → runtime executes the minimum.
2. **Config is IR, runtime is the compiled artifact.** User-facing JSON compiles into an immutable `FlowGraph`. The runtime walks the graph; it does not re-evaluate configuration.
3. **Declarative over procedural.** Users write match predicates. Hooks are a compilation output.
4. **Graph validity at load time.** Every path in every FlowGraph reaches a Terminator. Enforced at compile; runtime never has to handle "no matching rule."
5. **Zero-copy where the abstraction allows.** `Bytes` passthrough for bodies. TLS/HPACK/QPACK internal copies are out of scope — we promise zero-copy from the moment a stream unit reaches our engine.
6. **No `dyn Any`, no string-keyed KV.** Typed `http::Extensions` for every protocol-specific field.

## Non-goals that might look like goals

- **Cloudflare feature parity**. We borrow ideas (flow-based config, WASM isolation). We do not compete on scale, breadth, or edge-deployment features.
- **Nginx-replacement**. Vane's configuration model is rule compilation, not directive cascades. Porting an `nginx.conf` is not a supported workflow.
- **Protocol research**. No custom wire formats, no novel HTTP extensions. Ride on `http`, `hyper`, `h3`, `quinn`.
