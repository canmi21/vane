//! Stateless L7 middleware: injects `X-Forwarded-For` (append) and
//! `X-Real-IP` (overwrite) derived from `ConnContext.remote`.
//!
//! Off by default at the raw-rule layer; the `reverse_proxy` preset
//! enables it.
//!
//! See `spec/architecture/04-middleware.md` § _Stateless internal_ and
//! `spec/architecture/14-presets.md` § _`reverse_proxy`_.
