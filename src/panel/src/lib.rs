// Panel crate placeholder — SeamJS implementation removed, pending rebuild.
//
// The previous implementation exposed four procedures via SeamServer + Axum:
//
// listConnections (procedure, read-only)
//   - Calls engine.conn_registry().snapshot()
//   - Maps each ConnectionState to a flat struct with id, peer_addr, server_addr,
//     listen_port, layer (L4/L5/L7), phase (Accepted/Detecting/Forwarding/TlsHandshake),
//     protocol, tls_sni, tls_version, forward_target, started_at_unix_ms
//   - Returns { total, connections }
//
// getConfig (procedure, read-only)
//   - Serializes engine.current_config() to JSON via serde_json::to_value
//   - Returns { config: <raw JSON> }
//
// updateConfig (command, mutating)
//   - Deserializes input JSON into ConfigTable
//   - Calls engine.update_config(config)
//   - On success: { ok: true, validation_errors: [], error: null }
//   - On ConfigInvalid: { ok: false, validation_errors: [...], error: null }
//   - On other error: { ok: false, validation_errors: [], error: "..." }
//   - ValidationIssue has port, layer, step_path, message
//
// getSystemInfo (procedure, read-only)
//   - Returns version (CARGO_PKG_VERSION), started_at_unix_ms, listener_ports,
//     total_connections, configured_ports (sorted)
//
// Shared state was VaneState { engine: Arc<Engine>, started_at: SystemTime }.
// Instant-to-unix-ms conversion: elapsed = Instant::elapsed(), then SystemTime::now() - elapsed.
