# Task 2.15: Replace unwrap() and expect() in Production Code

This document tracks the audit and replacement of unsafe panicking calls in the Vane codebase.

**Goal:** Zero unexpected panics in the data plane.

---

## 🔴 Level 1: High Risk (Data Plane / Drivers)
*Required for production stability. Failure here drops connections.*

- [x] `src/modules/stack/protocol/carrier/quic/quic.rs:160`: `sni_found.unwrap()`
- [x] `src/modules/stack/protocol/application/http/httpx.rs:214`: `Response::builder()...unwrap()`
- [x] `src/modules/plugins/drivers/exec.rs:95,96`: `child.stdout.take().expect(...)`
- [x] `src/modules/plugins/terminator/response/mod.rs:214`: `HeaderValue::from_str(mime).unwrap()`
- [x] `src/modules/plugins/terminator/response/mod.rs:237`: `Response::builder()...unwrap()`
- [x] `src/modules/plugins/upstream/quic_pool.rs:43`: `parse().unwrap()` and `.expect(...)`

---

## 🟠 Level 2: Medium Risk (Management API / Bootstrap)
*Improves diagnostics and prevents startup crashes.*

- [x] `src/core/bootstrap.rs:211`: `TcpListener::bind(addr).await.unwrap()`
  - *Result:* Gracefully logs and exits if port is busy.
- [x] `src/modules/plugins/cgi/executor.rs:104,105,106`: `child.stdin.take().expect(...)`
  - *Result:* Kills child and returns Error::System.
- [x] `src/modules/plugins/resource/static.rs`: `HeaderValue::from_str(...).unwrap()`
  - *Result:* Returns anyhow::Result instead of panicking.
- [x] `src/modules/certs/loader.rs:111`: `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()`
  - *Result:* Replaced with `.unwrap_or_default()`.
- [x] `src/modules/stack/protocol/carrier/quic/muxer.rs:186`: `try_into().unwrap()`
  - *Result:* Handled conversion result safely.

---

## 🟡 Level 3: Low Risk (Invariants / Statics)
*Acceptable use cases, but can be improved for clarity.*

- [ ] `src/modules/nodes/model.rs:12`: `Regex::new(...).unwrap()`
- [ ] `src/modules/stack/protocol/carrier/quic/virtual_socket.rs:74`: `lock().unwrap()`

---

## 🟢 Level 4: Safe (Unit Tests)
*No action needed.*

- [ ] All code inside `#[cfg(test)]`.

---

## Progress Summary
- Total items: ~90
- Production risk items: 12
- Completed: 11