//! vane management protocol: wire format + verb schemas + server + client.
//!
//! See [`spec/crates/mgmt.md`](../../../spec/crates/mgmt.md).

pub mod client;
pub mod http_client;
pub mod http_server;
pub mod protocol;
pub mod server;
pub mod verb;

pub use client::{MgmtClientError, UnixMgmtClient};
pub use http_client::HttpMgmtClient;
pub use http_server::{HttpServerConfig, HttpServerError, spawn_http_server};
pub use protocol::{
	EndMarker, Request, Response, ResponseOutcome, WireError, WireErrorKind, encode_line,
};
pub use server::{DispatchOutcome, EventStream, Handler, spawn_unix_server};
