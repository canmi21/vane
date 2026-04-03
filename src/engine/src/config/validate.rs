use std::fmt;
use std::net::IpAddr;

use super::ConfigTable;

/// A single validation failure with location context.
#[derive(Debug, Clone)]
pub struct ValidationError {
	pub port: Option<u16>,
	pub message: String,
}

impl fmt::Display for ValidationError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		if let Some(port) = self.port {
			write!(f, "port {port}: ")?;
		}
		write!(f, "{}", self.message)
	}
}

impl ConfigTable {
	/// Validate the entire config table.
	pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
		let mut errors = Vec::new();

		for (&port, port_config) in &self.ports {
			let target = &port_config.target;

			if target.ip.parse::<IpAddr>().is_err() {
				errors.push(ValidationError {
					port: Some(port),
					message: format!("target.ip {:?} is not a valid IP address", target.ip),
				});
			}

			if target.port == 0 {
				errors.push(ValidationError {
					port: Some(port),
					message: "target.port must not be 0".to_owned(),
				});
			}
		}

		if errors.is_empty() { Ok(()) } else { Err(errors) }
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use std::collections::HashMap;

	use super::*;
	use crate::config::{GlobalConfig, ListenConfig, PortConfig, TargetAddr};

	fn simple_config(ports: HashMap<u16, PortConfig>) -> ConfigTable {
		ConfigTable { ports, global: GlobalConfig::default() }
	}

	fn simple_port(ip: &str, port: u16) -> PortConfig {
		PortConfig { listen: ListenConfig::default(), target: TargetAddr { ip: ip.to_owned(), port } }
	}

	#[test]
	fn valid_config() {
		let config = simple_config(HashMap::from([(80, simple_port("127.0.0.1", 8080))]));
		assert!(config.validate().is_ok());
	}

	#[test]
	fn empty_ports_allowed() {
		let config = simple_config(HashMap::new());
		assert!(config.validate().is_ok());
	}

	#[test]
	fn invalid_ip() {
		let config = simple_config(HashMap::from([(80, simple_port("not-an-ip", 8080))]));
		let errors = config.validate().unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("not a valid IP")));
	}

	#[test]
	fn zero_target_port() {
		let config = simple_config(HashMap::from([(80, simple_port("127.0.0.1", 0))]));
		let errors = config.validate().unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("must not be 0")));
	}

	#[test]
	fn ipv6_valid() {
		let config = simple_config(HashMap::from([(80, simple_port("::1", 8080))]));
		assert!(config.validate().is_ok());
	}

	#[test]
	fn multiple_errors_collected() {
		let config = simple_config(HashMap::from([
			(80, simple_port("bad-ip", 8080)),
			(81, simple_port("127.0.0.1", 0)),
		]));
		let errors = config.validate().unwrap_err();
		assert!(errors.len() >= 2);
	}

	#[test]
	fn validation_error_display() {
		let err = ValidationError { port: Some(443), message: "test error".to_owned() };
		assert_eq!(err.to_string(), "port 443: test error");
	}

	#[test]
	fn validation_error_display_no_port() {
		let err = ValidationError { port: None, message: "test error".to_owned() };
		assert_eq!(err.to_string(), "test error");
	}
}
