//! Free-port allocator: bind `:0`, read the assigned port, hand it back
//! to the system under test. Race between read and re-bind is accepted;
//! `listenfd` is deferred.
//!
//! See [`spec/conventions.md` § _Testing_](../../../spec/conventions.md#testing).

use std::net::TcpListener;

/// Allocate an ephemeral loopback TCP port by binding `127.0.0.1:0`,
/// reading the OS-assigned port, and releasing the socket. The window
/// between release and the caller re-binding is an accepted race — fine
/// for tests that bind promptly and rare enough to ignore in practice.
pub fn free_port() -> u16 {
	TcpListener::bind("127.0.0.1:0")
		.expect("bind ephemeral port")
		.local_addr()
		.expect("read local_addr")
		.port()
}
