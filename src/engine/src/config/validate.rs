use std::fmt;
use std::net::IpAddr;

use super::{ConfigTable, compile_rules};

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

		// Validate listener rules can compile
		if let Err(e) = compile_rules(&self.listeners) {
			errors.push(ValidationError { message: e.to_string() });
		}

		// Validate target if present
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
	use crate::config::{GlobalConfig, ListenerRule, Protocol, TargetAddr};

	fn config_with_listener(port: &str) -> ConfigTable {
		ConfigTable {
			listeners: vec![ListenerRule {
				bind: "0.0.0.0".to_owned(),
				port: port.to_owned(),
				protocol: Protocol::Tcp,
			}],
			target: Some(TargetAddr { ip: "127.0.0.1".to_owned(), port: 8080 }),
			global: GlobalConfig::default(),
		}
	}

	#[test]
	fn valid_config() {
		assert!(config_with_listener("8080").validate().is_ok());
	}

	#[test]
	fn empty_config_allowed() {
		assert!(ConfigTable::default().validate().is_ok());
	}

	#[test]
	fn invalid_listener_rule() {
		let config = config_with_listener("abc");
		let errors = config.validate().unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("not a valid u16")));
	}

	#[test]
	fn invalid_target_ip() {
		let config = ConfigTable {
			listeners: vec![],
			target: Some(TargetAddr { ip: "bad-ip".to_owned(), port: 8080 }),
			global: GlobalConfig::default(),
		};
		let errors = config.validate().unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("not a valid IP")));
	}

	#[test]
	fn zero_target_port() {
		let config = ConfigTable {
			listeners: vec![],
			target: Some(TargetAddr { ip: "127.0.0.1".to_owned(), port: 0 }),
			global: GlobalConfig::default(),
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
