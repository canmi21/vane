//! Built-in middleware implementations + registration against the
//! `MiddlewareMetadataProvider` / `MiddlewareFactories` surfaces.
//!
//! Catalog:
//! - L7 stateless: `host_header_match`, `path_prefix`, `method_match`,
//!   `forward_client_ip`.
//! - L7 stateful: `rate_limit`.
//! - L4 peek: `sni_peek`.
//!
//! See [`spec/crates/engine.md` § _Middleware_](../../../spec/crates/engine.md#middleware).

pub mod forward_client_ip;
pub mod host_header_match;
pub mod method_match;
pub mod path_prefix;
pub mod rate_limit;
pub mod sni_peek;
