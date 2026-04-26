//! Built-in middleware implementations + registration against the
//! `MiddlewareMetadataProvider` / `MiddlewareFactories` surfaces.
//!
//! Stage 1 L7 stateless set: `host_header_match`, `path_prefix`,
//! `method_match`, `forward_client_ip`. `rate_limit` (L2 stateful) lands
//! in Stage 2.
//!
//! See `spec/architecture/04-middleware.md` § _Internal middleware_.
//! Feature: S1-21.

pub mod forward_client_ip;
pub mod host_header_match;
pub mod method_match;
pub mod path_prefix;
pub mod rate_limit;
