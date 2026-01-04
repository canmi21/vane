/* src/common/net/portool.rs */

/// Checks if a number is a valid port number (1-65535).
pub fn is_valid_port(port: u16) -> bool {
	port > 0
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Tests the port validation logic with various inputs.
	#[test]
	fn test_port_validation() {
		// Test valid ports
		assert!(is_valid_port(1)); // Lower boundary
		assert!(is_valid_port(8080)); // Common port
		assert!(is_valid_port(65535)); // Upper boundary

		// Test invalid port (0)
		assert!(!is_valid_port(0));
	}
}
