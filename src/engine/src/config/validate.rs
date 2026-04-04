use std::fmt;
use std::net::IpAddr;

use super::ConfigTable;

/// A single validation failure with location context.
#[derive(Debug, Clone)]
pub struct ValidationError {
	pub message: String,
}

impl fmt::Display for ValidationError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.message)
	}
}

impl ConfigTable {
	/// Validate the entire config table.
	pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
		let mut errors = Vec::new();

		for (i, entry) in self.listeners.iter().enumerate() {
			if entry.bind.parse::<IpAddr>().is_err() {
				errors.push(ValidationError {
					message: format!("listener #{}: bind {:?} is not a valid IP", i, entry.bind),
				});
			}
		}

		if let Some(target) = &self.target {
			if target.ip.parse::<IpAddr>().is_err() {
				errors.push(ValidationError {
					message: format!("target.ip {:?} is not a valid IP address", target.ip),
				});
			}
			if target.port == 0 {
				errors.push(ValidationError { message: "target.port must not be 0".to_owned() });
			}
		}

		if errors.is_empty() { Ok(()) } else { Err(errors) }
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;
	use crate::config::{CompiledListener, SingleProtocol, TargetAddr};

	fn tcp_listener(bind: &str, port: u16) -> CompiledListener {
		CompiledListener { bind: bind.to_owned(), port, protocol: SingleProtocol::Tcp }
	}

	#[test]
	fn valid_config() {
		let config = ConfigTable {
			listeners: vec![tcp_listener("0.0.0.0", 8080)],
			target: Some(TargetAddr { ip: "127.0.0.1".to_owned(), port: 8080 }),
			..Default::default()
		};
		assert!(config.validate().is_ok());
	}

	#[test]
	fn empty_config_allowed() {
		assert!(ConfigTable::default().validate().is_ok());
	}

	#[test]
	fn invalid_listener_bind() {
		let config = ConfigTable {
			listeners: vec![tcp_listener("bad-ip", 8080)],
			target: None,
			..Default::default()
		};
		let errors = config.validate().unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("not a valid IP")));
	}

	#[test]
	fn invalid_target_ip() {
		let config = ConfigTable {
			listeners: vec![],
			target: Some(TargetAddr { ip: "bad-ip".to_owned(), port: 8080 }),
			..Default::default()
		};
		let errors = config.validate().unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("not a valid IP")));
	}

	#[test]
	fn zero_target_port() {
		let config = ConfigTable {
			listeners: vec![],
			target: Some(TargetAddr { ip: "127.0.0.1".to_owned(), port: 0 }),
			..Default::default()
		};
		let errors = config.validate().unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("must not be 0")));
	}

	#[test]
	fn validation_error_display() {
		let err = ValidationError { message: "test error".to_owned() };
		assert_eq!(err.to_string(), "test error");
	}
}
