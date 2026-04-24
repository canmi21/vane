//! `VanedFixture` — owns a tmp config dir + Unix-socket path + spawned
//! `vaned` child process, waits for socket-ready, auto-teardowns on
//! `Drop`. Drives end-to-end tests.
//!
//! See `spec/testing.md` § _Test surface by binary kind_ and § _Fixture
//! management_.
