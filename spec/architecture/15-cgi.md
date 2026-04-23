# CGI Driver

CGI is the sole non-socket-based upstream. Every request fork-execs a new process, pipes the request body to stdin, and parses the child's stdout as an RFC 3875 response. Structurally distinct from all other Fetch paths.

## Process model: per-request fork-exec, no pool

Per spec, CGI is inherently per-request fork-exec. "Pooling" would require a different protocol (FastCGI, SCGI, WSGI) — explicitly out of scope.

Each `Fetch::HttpProxy { upstream: HttpUpstream::Cgi { ... } }` invocation:

1. `fork` child
2. In child: set env vars, working directory, uid/gid, rlimits
3. `exec` binary
4. Parent writes request body → child stdin; closes stdin on EOF
5. Parent reads child stdout; parses as RFC 3875 response
6. Wait for child exit; collect exit code
7. Clean up fds

Cost is real — fork + exec is ~1 ms on Linux, plus the binary's own startup (tens of ms for Python / Ruby). Users opt into CGI deliberately (legacy integration, small scripts), accepting the per-request cost.

## Environment variables

CGI environment is constructed explicitly; **the daemon's own environment is not inherited**. This prevents unintended leakage of secrets living in the daemon's env (e.g., `AWS_ACCESS_KEY`, `DATABASE_URL`).

### Required by RFC 3875

```
CONTENT_LENGTH          request body size in bytes ("0" if no body)
CONTENT_TYPE            request body's Content-Type header
GATEWAY_INTERFACE       "CGI/1.1"
PATH_INFO               path after the script name (see below)
QUERY_STRING            URL query string (without '?')
REMOTE_ADDR             client IP
REMOTE_HOST             client hostname (for simplicity, = REMOTE_ADDR)
REQUEST_METHOD          HTTP method
SCRIPT_NAME             URL prefix that maps to the script
SERVER_NAME             Host header, or listener's configured name
SERVER_PORT             listener's port
SERVER_PROTOCOL         "HTTP/1.1" (even if client is H2/H3 — we downgrade)
SERVER_SOFTWARE         "vane/<version>"

HTTP_<UPPERCASE_HEADER>   every request header, normalized
                          (except Content-Type, Content-Length, which have dedicated vars)
                          e.g., User-Agent → HTTP_USER_AGENT
```

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

User-provided `env` entries merge into the CGI process's environment. User keys may override computed values (e.g., overriding `SERVER_SOFTWARE`). Final env = computed vars ∪ user-provided (user wins on key conflict).

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
    pub uid:    Option<u32>,        // drop to this uid via setuid; None = same as daemon
    pub gid:    Option<u32>,        // drop to this gid via setgid
    pub limits: ResourceLimits,
}

pub struct ResourceLimits {
    pub memory_mb:     Option<u64>,  // RLIMIT_AS
    pub cpu_seconds:   Option<u64>,  // RLIMIT_CPU
    pub max_processes: Option<u64>,  // RLIMIT_NPROC (typically 1; no forking)
}
```

- **uid/gid** — requires daemon to run with `CAP_SETUID` / `CAP_SETGID`. Without those capabilities, attempting to switch is a spawn failure logged with a specific error.
- **rlimits** — enforced by the kernel. Exceeding any kills the child; we see non-zero exit, return `502`.
- **chroot** — not in MVP. Architectural slot reserved for post-MVP via `chroot: Option<PathBuf>` on `CgiSecurity`.

## Concurrency cap

Daemon-global `max_concurrent_cgi_processes`, default **100**, configurable via env:

```
VANE_CGI_MAX_CONCURRENT=100
```

When the cap is reached, new CGI requests return **503 Service Unavailable** immediately — no queueing. Queueing under sustained overload amplifies resource pressure (each queued request still holds its connection + request state); fast rejection surfaces the overload to operators.

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
    binary:      PathBuf,
    script_name: String,                       // required; no filesystem walk
    env:         Vec<(String, String)>,        // user-defined, merged with computed
    security:    CgiSecurity,
    working_dir: Option<PathBuf>,              // defaults to binary's parent dir
}
```
