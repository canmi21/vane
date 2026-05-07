//! Echo HTTP / TCP servers with auto-teardown on `Drop` (`EchoHandle`).
//! UDP echo + TLS fixtures land with their respective protocol features.
//!
//! See [`spec/conventions.md` § _Testing_](../../../spec/conventions.md#testing).
//
// TODO(testutil-echo): module is documentation-only; integration tests
// currently inline equivalents. Land the shared helpers here when more
// than one test needs the same shape.
