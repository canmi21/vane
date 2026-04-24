//! Free-port allocator: bind `:0`, read the assigned port, hand it back
//! to the system under test. Race between read and re-bind is accepted;
//! `listenfd` is deferred.
//!
//! See `spec/testing.md` § _Fixture management_.
