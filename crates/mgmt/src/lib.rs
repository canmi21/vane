//! vane management protocol: vane-specific verb schemas plus
//! re-exports of the project-agnostic NDJSON-RPC framing
//! ([`ndjson_rpc`]).
//!
//! See [`spec/crates/mgmt.md`](../../../spec/crates/mgmt.md).

pub mod verb;

// `ndjson_rpc` 0.0.3 dropped the `Mgmt`-prefixed type names so its
// public surface is project-agnostic; vane-mgmt is the canonical
// "management plane" consumer in this workspace, so we re-export
// under the original project-specific names. Downstream vane code
// imports `MgmtClientError` / `UnixMgmtClient` / `HttpMgmtClient`
// from this crate as before.
pub use ndjson_rpc::{
	ClientError as MgmtClientError, DispatchOutcome, EndMarker, EventStream, Handler,
	HttpClient as HttpMgmtClient, HttpServerConfig, HttpServerError, Request, Response,
	ResponseOutcome, UnixClient as UnixMgmtClient, WireError, WireErrorKind, encode_line,
	spawn_http_server, spawn_unix_server,
};
pub use ndjson_rpc::{client, http_client, http_server, protocol, server};
