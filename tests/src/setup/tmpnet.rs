/* tests/src/setup/tmpnet.rs */

use rand::Rng;
use std::net::{TcpListener, UdpSocket};

const MAX_PORT_FIND_ATTEMPTS: u32 = 100;

/// Checks if a given network port is already in use for a specific protocol.
///
/// This function attempts to bind a socket to `127.0.0.1` on the specified
/// port. A failure to bind is interpreted as the port being taken.
///
/// # Arguments
///
/// * `protocol` - The protocol to check, either "tcp" or "udp".
/// * `port` - The `u16` port number to check.
///
/// # Panics
///
/// Panics if an unsupported protocol is provided.
pub fn is_port_taken(protocol: &str, port: u16) -> bool {
	let address = format!("127.0.0.1:{}", port);
	match protocol {
		"tcp" => TcpListener::bind(&address).is_err(),
		"udp" => UdpSocket::bind(&address).is_err(),
		_ => panic!("Unsupported protocol specified: {}", protocol),
	}
}

/// Finds an available network port for a given protocol.
///
/// This function repeatedly selects a random port from the dynamic/private
/// range (49152-65535) and attempts to bind to it until it finds one that
/// is not in use.
///
/// # Arguments
///
/// * `protocol` - The protocol for which to find a port, either "tcp" or "udp".
///
/// # Returns
///
/// A `u16` port number that is currently available.
///
/// # Panics
///
/// Panics if it cannot find an available port after a fixed number of attempts,
/// or if an unsupported protocol is provided.
pub fn find_available_port(protocol: &str) -> u16 {
	for _ in 0..MAX_PORT_FIND_ATTEMPTS {
		// Use the IANA recommended ephemeral port range
		let port: u16 = rand::rng().random_range(49152..=65535);
		if !is_port_taken(protocol, port) {
			return port;
		}
	}

	panic!(
		"Failed to find an available {} port after {} attempts.",
		protocol, MAX_PORT_FIND_ATTEMPTS
	);
}
