//! Tracing sink for tests: captures events into an in-memory sink so
//! assertions can read emitted `kind` / `reason` fields directly.
//!
//! See [`spec/conventions.md` § _Testing_](../../../spec/conventions.md#testing).
//
// TODO(testutil-tracing): module is documentation-only; integration
// tests currently rely on `tracing-subscriber`'s default fmt subscriber
// or roll their own in-memory sinks. Land the shared sink here when a
// stable interface is needed.
