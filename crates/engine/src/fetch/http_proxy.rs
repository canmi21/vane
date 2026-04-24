//! `HttpProxyFetch` — the reverse-proxy Fetch. Stage 1 ships the H1→H1
//! path via `hyper_util::client::legacy::Client`. H2/H3 client-side and
//! upstream TLS land in later stages.
//!
//! See `spec/architecture/05-terminator.md` § _`HttpProxy`_ and
//! `spec/architecture/07-l7.md`. Feature: S1-19.
