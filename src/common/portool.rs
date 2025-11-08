/* src/common/portool.rs */

/// Checks if a number is a valid port number (1-65535).
pub fn is_valid_port(port: u16) -> bool {
	port > 0
}
