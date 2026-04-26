//! vane management protocol: wire format + verb schemas + server + client.
//!
//! See `spec/architecture/10-management.md`.

pub mod client;
pub mod protocol;
pub mod server;
pub mod verb;

pub use client::{MgmtClientError, UnixMgmtClient};
pub use protocol::{
	EndMarker, Request, Response, ResponseOutcome, WireError, WireErrorKind, encode_line,
};
pub use server::{DispatchOutcome, EventStream, Handler, spawn_unix_server};
