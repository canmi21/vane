# CGI Driver

CGI is the sole non-socket-based upstream. Every request fork-execs a new process, pipes the request body to stdin, and parses the child's stdout as an RFC 3875 response. Structurally distinct from all other Fetch paths.

## Process model: per-request fork-exec, no pool

Per spec, CGI is inherently per-request fork-exec. "Pooling" would require a different protocol (FastCGI, SCGI, WSGI) — explicitly out of scope.

Each `HttpProxyFetch { upstream: HttpUpstream::Cgi { ... }, ... }` invocation uses `tokio::process::Command` configured with:

- `env_clear()` then `envs(computed_rfc3875_vars)` — no daemon env inherited.
- `current_dir(working_dir)`.
- `stdin(Stdio::piped())`, `stdout(Stdio::piped())`, `stderr(Stdio::piped())`.
- [`std::os::unix::process::CommandExt::pre_exec`](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html) — a closure that runs **after `fork`, before `exec`** in the child process. The closure issues the async-signal-safe syscalls: `setgid`, `setuid`, `setrlimit` for each configured rlimit, optional `chroot`. Errors in the closure are returned as `io::Error` to the parent side of `spawn()`.
- `spawn()` then await the child's exit.

Steps end-to-end:

1. `spawn()` → kernel `fork` + pre_exec closure runs in child + `exec` binary.
2. Parent writes request body → child stdin; closes stdin on EOF.
3. Parent reads child stdout; parses as RFC 3875 response.
4. Wait for child exit; collect exit code.
5. Clean up fds.

`pre_exec` requires `unsafe`. The workspace lints forbid `unsafe_code` by default (`16-crate-layout.md`), so the CGI module carries a reviewed `#[allow(unsafe_code)]` with a comment documenting the async-signal-safety discipline of the closure body — **no allocations, no mutex locks, no file I/O beyond the listed syscalls**. Commit metadata records the auditor.

Cost is real — fork + exec is ~1 ms on Linux, plus the binary's own startup (tens of ms for Python / Ruby). Users opt into CGI deliberately (legacy integration, small scripts), accepting the per-request cost.

## Environment variables

CGI environment is constructed explicitly; **the daemon's own environment is not inherited**. This prevents unintended leakage of secrets living in the daemon's env (e.g., `AWS_ACCESS_KEY`, `DATABASE_URL`).

### Required by RFC 3875

```
CONTENT_LENGTH          request body size in bytes ("0" if no body)
CONTENT_TYPE            request body's Content-Type header
GATEWAY_INTERFACE       "CGI/1.1"
PATH_INFO               path after the script name (see below)
PATH_TRANSLATED         canonical(working_dir + PATH_INFO) — RFC 3875 §4.1.6
QUERY_STRING            URL query string (without '?')
REMOTE_ADDR             client IP
REMOTE_HOST             client hostname (for simplicity, = REMOTE_ADDR)
REQUEST_METHOD          HTTP method
SCRIPT_NAME             URL prefix that maps to the script
SERVER_NAME             Host header, or listener's configured name
SERVER_PORT             listener's port
SERVER_PROTOCOL         "HTTP/1.1" (even if client is H2/H3 — we downgrade)
SERVER_SOFTWARE         "vane/<version>"

HTTP_<UPPERCASE_HEADER>   every request header except those in `block_headers`
                          and the dedicated vars (Content-Type, Content-Length)
                          e.g., User-Agent → HTTP_USER_AGENT
```

#### Header passthrough — `block_headers`

The set of headers that map to `HTTP_*` vars is filtered through a per-rule `block_headers` list. The list is **required**: the JSON config must specify it explicitly, and the CLI / TUI emits a safe default of `["Authorization", "Cookie", "Proxy-Authorization"]` so operators see what is being blocked rather than discovering it through reading source.

`Authorization` and `Cookie` carry credentials whose appearance in a child process's environment exposes them via `/proc/<pid>/environ`, in `printenv` debugging output, and in the environments of any sub-processes the CGI script forks. `Proxy-Authorization` carries the same risk for proxy-aware setups. Operators can replace the list with anything else (including the empty list) but the absence of the field is a compile error — there is no implicit default.

CGI rules wanting to expose `Authorization` to a CGI script (legacy auth-aware CGI) write `block_headers: ["Cookie", "Proxy-Authorization"]` explicitly.

### Common extensions, always set

Not in RFC 3875 but ubiquitous in modern CGI-adjacent code:

```
REMOTE_PORT             client port
REQUEST_URI             full URI (with query string)
REQUEST_SCHEME          "http" or "https"
HTTPS                   "on" when the request is HTTPS; otherwise unset
DOCUMENT_URI            decoded URI path
```

### User-defined, per rule

```json
{
	"type": "cgi",
	"binary": "/var/www/cgi-bin/app.cgi",
	"script_name": "/cgi-bin/app.cgi",
	"env": {
		"DATABASE_URL": "postgres://...",
		"APP_MODE": "production"
	}
}
```

User-provided `env` entries merge into the CGI process's environment for **non-reserved** keys. The reserved set is the union of:

- Every RFC 3875 required variable name listed above.
- Every common-extension variable name listed in § _Common extensions, always set_.
- Every `HTTP_*` form derived from a request header.

A user `env` entry whose key is in the reserved set is a **compile-time error**. Reason: vane computes these values per request from connection state, and operators silently overriding them produces CGI scripts that read confidently-wrong data (e.g. `REQUEST_METHOD = "FAKE"`, `REMOTE_ADDR = "0.0.0.0"`). Use cases that look like overrides — propagating a real client IP through a load balancer, for example — go through L7 middleware (`forward_client_ip`) updating `ConnContext`, after which CGI sees the corrected `REMOTE_ADDR` automatically.

Final env = computed vars ∪ user-provided (no overlap permitted).

### Isolation from daemon env

The daemon's own env (loaded by `dotenvy` at startup, including secrets) is **not propagated** to CGI children. The CGI child process's environment contains _only_ what this subsystem constructs — RFC 3875 vars, common extensions, user-declared env. A deliberate boundary to keep CGI scripts from accidentally reading daemon secrets.

## Path handling: explicit `script_name`

The request URI is split into `SCRIPT_NAME` and `PATH_INFO` based on the rule's explicit `script_name` field — **not** via filesystem walking (which is how Apache mod_cgi does it, and is fragile).

For request `GET /cgi-bin/app.cgi/users/42?sort=asc` with rule `script_name: "/cgi-bin/app.cgi"`:

```
SCRIPT_NAME   = /cgi-bin/app.cgi
PATH_INFO     = /users/42
QUERY_STRING  = sort=asc
```

If the request URI doesn't begin with `script_name`, the rule should not match (path-prefix predicate on `script_name` is typically the right predicate; the rule author's responsibility).

## stdin / stdout protocol

### stdin

Request body raw bytes followed by EOF (we close the stdin fd). No chunking, no framing — raw bytes.

### stdout

Parsed as RFC 3875 response:

```
<header-name>: <value>\r\n
<header-name>: <value>\r\n
\r\n
<body bytes until EOF>
```

## Streaming posture: half-buffered

CGI does **not** participate in the "both sides native streaming" posture of `07-l7.md`. It is a half-buffered path by protocol constraint:

- **Request side**: vane writes the request body to the child's stdin as bytes arrive from the client decoder. This is structurally streaming from vane's view, but the CGI process model (RFC 3875) requires the child to see **stdin EOF** before producing output. Typical CGI scripts read stdin fully before writing anything. The path is therefore observationally equivalent to "request is buffered at the child". `max_body_size` on the request side is enforced during the write loop; exceeding it `SIGTERM`s the child and returns `413 Payload Too Large`.
- **Response side**: the child's stdout is read frame by frame. After the RFC 3875 header block (terminated by `\r\n\r\n`), every stdout `read()` becomes a `Body::Stream` frame handed to the response encoder. No vane-side buffering on this side.

LazyBuffer analysis sees the CGI `L7Fetch` node like any other: if a response-side middleware declares `needs_body()`, the response side buffers as usual (on top of the per-frame stream already flowing out of stdout). The request-side "buffering at the child" is below the Fetch abstraction — it does not appear in the graph and is not controlled by LazyBuffer flags.

Rules using CGI implicitly accept this posture. Retry on CGI is not supported (the child is a one-shot process by RFC 3875; "replay" would require re-forking, which changes the child PID and breaks any PID-keyed external state).

Special headers:

- `Status: 200 OK` — sets HTTP status code. This is a CGI-specific header, not an HTTP/1.1 status line.
- `Location: /other` — without a `Status` header, sets status = 302.
- All other headers — passed through to the client.

Body bytes begin after `\r\n\r\n` and continue until the child closes stdout (typically on process exit).

### Exit code

- `0` → normal completion; stdout output is the response.
- non-zero → treated as `502 Bad Gateway` to the client; exit code logged.

## Security

Per-rule configuration, enforced at spawn time:

```rust
pub struct CgiSecurity {
    pub uid:    u32,                // drop to this uid via setuid; required
    pub gid:    u32,                // drop to this gid via setgid; required
    pub limits: ResourceLimits,
    pub chroot: Option<PathBuf>,    // schema reserved; runtime unimplemented (see below)
}

pub struct ResourceLimits {
    pub memory_mb:     Option<u64>,  // RLIMIT_AS;     required field; null = no limit
    pub cpu_seconds:   Option<u64>,  // RLIMIT_CPU;    required field; null = no limit
    pub max_processes: Option<u64>,  // RLIMIT_NPROC;  required field; null = no limit
}
```

`uid` and `gid` are **required**. There is no "inherit from daemon" fallback — every CGI rule names the identity it runs as explicitly. The CLI / TUI prompts for these values during initial setup of a CGI rule and writes them out; absence is a compile error.

If the resolved `uid` is `0` (root) at boot time, vane emits a `WARN` log entry (`"cgi rule '<name>' configured to run as root; verify this is intended"`) but does **not** refuse to start. Container deployments where the daemon's view of "root" is namespaced commonly use uid 0 legitimately; a hard refusal would over-block. The required-field rule above already prevents the silent footgun (operator forgetting to set uid and inheriting daemon root) — explicit `uid: 0` is an informed choice.

`ResourceLimits` fields are all required. Each accepts an explicit `null` to mean "no limit" or an integer to mean "limit to this value". Absence is a compile error — operators must consciously decide whether each limit applies. CLI / TUI defaults:

| Field           | CLI / TUI default | Reasoning                                                                                                     |
| --------------- | ----------------- | ------------------------------------------------------------------------------------------------------------- |
| `memory_mb`     | `256`             | Most CGI scripts fit comfortably; bounds runaway memory.                                                      |
| `cpu_seconds`   | `30`              | Slightly under `total_timeout` so kernel-side cpu kill fires before vane-side timeout.                        |
| `max_processes` | `null`            | Setting low (e.g. `1`) breaks legitimate shell-out (git CGI, image converters); operator tightens explicitly. |

- **uid/gid** — requires daemon to run with `CAP_SETUID` / `CAP_SETGID`. Without those capabilities, attempting to switch is a spawn failure logged with a specific error.
- **rlimits** — enforced by the kernel. Exceeding any kills the child; we see non-zero exit, return `502`.
- **chroot** — schema field is reserved (`chroot: Option<PathBuf>` on `CgiSecurity`). The runtime does not implement chroot in MVP: a CGI rule with `chroot: Some(...)` fails compile with `"chroot is reserved but not yet implemented"`. Locking the field shape now keeps the JSON schema stable for the future post-MVP implementation pass; operators who need chroot today must wrap vane's CGI in an external sandboxing layer.

## Bootstrap validation

`vane compile` validates each CGI rule's `binary` path at compile time:

- The path must exist.
- The file must be executable by the configured `uid` (`access(2)` with `X_OK`, evaluated against the target uid's view).
- Failures are **rule-level compile errors**, not daemon-wide boot failures: other rules continue to compile.

This catches the most common operator misconfiguration (path typo, missing binary, wrong permissions) at reload time rather than at first CGI request. Network-mounted binaries that may be temporarily unavailable at startup must either be mounted before reload, or the operator deals with the rule-level compile error and reloads again once the mount completes — there is no "skip validation" escape valve.

## stderr handling

The CGI child's `stderr(Stdio::piped())` output is consumed line-by-line by the parent. Each line is emitted as a `tracing` event at `WARN` level with structured fields:

| Field          | Value                                |
| -------------- | ------------------------------------ |
| `event.target` | `"vane::cgi"`                        |
| `rule`         | rule name from the FlowGraph         |
| `binary`       | the `binary` path                    |
| `pid`          | child process PID                    |
| `message`      | the stderr line, UTF-8 lossy decoded |

`WARN` level (rather than `ERROR`) reflects the convention that CGI scripts use stderr for diagnostics that are not necessarily errors. Operators who want to filter CGI noise from their structured log can subscribe to `tail_log` with `event.target != "vane::cgi"`. There is no per-rule knob for stderr disposition — one default behavior is sufficient.

## Concurrency cap

Daemon-global `max_concurrent_cgi_processes`, default **100**, configurable via env:

```
VANE_CGI_MAX_CONCURRENT=100
```

When the cap is reached, new CGI requests return **503 Service Unavailable** immediately — no queueing. Queueing under sustained overload amplifies resource pressure (each queued request still holds its connection + request state); fast rejection surfaces the overload to operators.

The default value applies when `VANE_CGI_MAX_CONCURRENT` is unset. The "no implicit defaults" rule that governs rule-level JSON schema fields is intentionally not extended to environment variables: env vars are operational configuration set at startup (alongside `VANE_BIND_IPV4` etc.), not declarative configuration that the CLI / TUI emits. Requiring every env var to be set explicitly would only make daemons harder to start.

## Timeouts

CGI Fetch shares `HttpProxy`'s `timeouts`:

- `connect_timeout` (default `5s`) — fork+exec completion to first byte on stdout
- `total_timeout` (default `60s`) — overall request budget
- On timeout: `SIGTERM` to child; after 1s grace, `SIGKILL`; return `504 Gateway Timeout` to client

## `SERVER_ADDR` and `SERVER_PORT`

Derived from `ConnContext.local` — the listener address the client actually connected to, not the rule's listener config. CGI scripts generating self-referential URLs produce correct addresses even when the daemon binds multiple addresses.

## Full `HttpUpstream::Cgi` shape

```rust
HttpUpstream::Cgi {
    binary:        PathBuf,
    script_name:   String,                     // no filesystem walk
    working_dir:   PathBuf,                    // required; CLI/TUI emits binary's parent dir
    env:           Vec<(String, String)>,      // user-defined, no overlap with reserved keys
    block_headers: Vec<String>,                // required; CLI/TUI emits the safe-default list
    security:      CgiSecurity,
}
```

Every field is required at the schema level (the JSON config must include each by name). `env` may be the empty list. `block_headers` may be the empty list, but the field's presence is mandatory — the CLI / TUI emits `["Authorization", "Cookie", "Proxy-Authorization"]` by default so an absent value is conspicuous in review.
