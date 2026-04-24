//! Middleware trait contracts: `L4PeekMiddleware`, `L4BytesMiddleware`,
//! `L7RequestMiddleware`, `L7ResponseMiddleware`, `Decision`, `ShortCircuit`,
//! `CloseReason`, and the symbolic `MiddlewareKind` + `SymbolicMiddlewareRef`
//! the IR uses before link.
//!
//! `MiddlewareInst` (the linked, trait-object form) lives in `vane-engine`.
//!
//! See `spec/architecture/04-middleware.md`. Feature: S1-05.
