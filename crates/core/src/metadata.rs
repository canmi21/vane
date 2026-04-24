//! Metadata-provider traits consumed by the core compile pipeline:
//! `MiddlewareMetadataProvider`, `FetchMetadataProvider`, plus the
//! descriptor types (`MiddlewareMetadata`, `FetchMetadata`).
//!
//! Engine registers built-ins against these; WASM module load registers
//! plugins. Core never sees concrete impls — only the provider trait.
//!
//! See `spec/architecture/04-middleware.md` § _Where the types live_.
//! Feature: S1-08.
