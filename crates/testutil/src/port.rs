//! Free-port allocator: bind `:0`, read the assigned port, hand it back
//! to the system under test. Race between read and re-bind is accepted;
//! `listenfd` is deferred.
//!
//! See [`spec/conventions.md` § _Testing_](../../../spec/conventions.md#testing).
//
// TODO(testutil-port): module is documentation-only; integration tests
// currently inline a `bind :0 → drop → reuse` helper. Land the shared
// implementation here when the duplication becomes load-bearing.
