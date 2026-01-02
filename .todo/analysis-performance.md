# Analysis: Performance Tuning

**Date:** 2026-01-02
**Context:** Phase IV Deep Analysis

## 1. Async Runtime
-   **Current:** `tokio::spawn` used liberally.
-   **Optimization:**
    -   Use `LocalSet` for non-Send futures where possible (e.g., within a specific listener thread) to avoid synchronization overhead? (Likely too complex for Vane's architecture).
    -   Tune Tokio worker threads based on CPU cores (`worker_threads` config).

## 2. Memory Allocations
-   **Hotspot:** `hex::encode` in `tls.rs` and `quic.rs` for KV injection.
    -   *Fix:* Store `Bytes` in KV instead of String? Or create a `LazyHex` type?
-   **Hotspot:** `Clone` of `datagram` in QUIC dispatcher.
    -   *Fix:* Use `Arc<Vec<u8>>` or `Bytes` for shared ownership of read-only packet data.

## 3. Flow Engine
-   **Optimization:** `HashMap` lookups in `KvStore` are O(1) but hashing has cost.
    -   *Fix:* Use `ahash` or `fxhash` for faster hashing than SipHash (std default).
    -   *Fix:* Pre-intern common keys (`conn.ip`, `req.path`) as integers or Enums?

## 4. UDP NAT
-   **Contention:** `SESSIONS` DashMap in `proxy.rs` is hit for every packet.
    -   *Fix:* Shard the map more aggressively?
    -   *Fix:* Use `socket2` `SO_REUSEPORT` and thread-local maps?
