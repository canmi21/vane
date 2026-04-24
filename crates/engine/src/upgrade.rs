//! `Node::Upgrade` execution — L4 → L7 bridge. Hands the TCP stream to
//! `hyper::server::conn::http1::Builder` (Stage 1) / `http2::Builder`
//! (Stage 2); each decoded `Request` walks the L7 sub-graph from the
//! `Upgrade.next` node.
//!
//! See `spec/architecture/06-l4.md` § _L4 → L7 upgrade_,
//! `spec/architecture/02-flow.md` § _Execution model_ (Upgrade arm).
//! Feature: S1-17.
