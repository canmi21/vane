# Analysis: Security & Vulnerabilities

**Date:** 2026-01-02
**Context:** Phase IV Deep Analysis

## 1. Logic Gaps & Risks

### 1.1 QUIC Amplification / Resource Exhaustion
-   **Risk:** `session::PENDING_INITIALS` buffers packets. A flood of fragmented Initial packets with random DCIDs could exhaust memory.
-   **Mitigation:**
    -   Enforce strict total byte limit on `PENDING_INITIALS`.
    -   Implement aggressive timeout (currently relies on `session::cleanup_task`).

### 1.2 TLS ClientHello Buffering
-   **Risk:** `TLS_CLIENTHELLO_BUFFER_SIZE` defaults to 4KB. Attackers sending valid TLS records > 4KB could bypass inspection or cause parse failures leading to fallback (which might be "allow").
-   **Mitigation:**
    -   Ensure failure to parse -> Drop connection (Fail Closed). Currently, it seems to log warning and proceed? (Need to verify `tls.rs`).

### 1.3 Unbounded Streams
-   **Risk:** `tokio::io::copy` in `proxy.rs` copies until EOF. Slowloris attacks or stalled connections can hold file descriptors indefinitely.
-   **Mitigation:**
    -   Wrap `copy` with `tokio::time::timeout` (idle timeout).
    -   Implement bandwidth rate limiting (Token Bucket).

### 1.4 L7 Buffer DoS
-   **Risk:** `L7_MAX_BUFFER_SIZE` (10MB) is global. Concurrent requests triggering `force_buffer()` (e.g., via `{{req.body}}` template) could OOM the server.
-   **Mitigation:**
    -   Global semaphore for total buffered bytes across all connections.

## 2. Panic Safety
-   **Status:** Most `unwrap()` calls removed in Phase II.
-   **Action:** Continue monitoring `TODO.md` list. Check `h3` implementation details (complex async logic often hides panics).

## 3. External Plugins
-   **Risk:** `env` map in `Command` driver allows passing `LD_PRELOAD`.
-   **Mitigation:** filter sensitive env vars (`LD_*`, `PATH` overrides) in `drivers/exec.rs`.
