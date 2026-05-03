//! `CgiFetch` — RFC 3875 CGI driver.
//!
//! Per `spec/architecture/15-cgi.md`, every request fork-execs a fresh
//! process, pipes the request body to its stdin, parses the child's
//! stdout as an RFC 3875 response, and emits stderr lines as `tracing`
//! events. The driver lives in its own module rather than the
//! `HttpProxyFetch` `Dispatch` enum because none of the socket-side
//! machinery (connection pool, retry, ALPN, upstream URI rewrite,
//! `connect_timeout` semantics) applies — fork+exec is a different
//! protocol with different invariants.
//!
//! Scaffold-only at this step: the factory is a stub that the
//! subsequent commit fills in. `fetch::http_proxy::factory` already
//! routes to `cgi::factory` when `args.upstream_kind == "cgi"`, so
//! linking a CGI rule today produces a clear "not yet implemented"
//! rule-level error rather than a misleading "missing upstream".

use crate::factories::FactoryError;
use crate::flow_graph::FetchInst;

/// Build a `CgiFetch` from the resolved rule args. Stub for now —
/// fields are validated, the binary is checked, and the runtime is
/// wired in the next commit. See `spec/architecture/15-cgi.md` §
/// _Bootstrap validation_ for the failure mode this layer catches.
///
/// # Errors
/// Returns [`FactoryError`] for every CGI rule until the runtime
/// lands.
pub fn factory(_args: &serde_json::Value) -> Result<FetchInst, FactoryError> {
	Err(FactoryError("CGI fetch driver is not yet implemented".to_string()))
}
