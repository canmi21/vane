//! `VanedFixture` — owns a tmp config dir + Unix-socket path + spawned
//! `vaned` child process, waits for socket-ready, auto-teardowns on
//! `Drop`. Drives end-to-end tests.
//!
//! See [`spec/conventions.md` § _Test surface by binary kind_](../../../spec/conventions.md#test-surface-by-binary-kind).
//
// TODO(testutil-vaned-fixture): module is documentation-only; daemon
// E2E tests currently spawn `vaned` directly via `assert_cmd`. Land
// the shared fixture (tmp dir + socket-ready wait + Drop teardown)
// here when more than one test needs the same shape.
