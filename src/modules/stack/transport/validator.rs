/* src/modules/stack/transport/validator.rs */

use std::collections::HashSet;
use validator::ValidationError;

use super::model::DetectMethod;
use super::tcp::{TcpDestination, TcpProtocolRule};
use super::udp::UdpProtocolRule;

pub fn validate_tcp_rules(rules: &[TcpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
		if rule.session.is_some() {
			if let TcpDestination::Forward { .. } = &rule.destination {
				let mut err = ValidationError::new("session_with_forward");
				err.message =
					Some("The 'session' block is only allowed for 'resolver' type destinations.".into());
				return Err(err);
			}
		}
		match rule.detect.method {
			DetectMethod::Regex => {
				if fancy_regex::Regex::new(&rule.detect.pattern).is_err() {
					let mut err = ValidationError::new("invalid_regex");
					err.message =
						Some(format!("Pattern '{}' is not a valid regex.", rule.detect.pattern).into());
					return Err(err);
				}
			}
			DetectMethod::Fallback => {
				if rule.detect.pattern != "any" {
					let mut err = ValidationError::new("invalid_fallback_pattern");
					err.message = Some("Pattern for 'fallback' method must be 'any'.".into());
					return Err(err);
				}
			}
			_ => {}
		}
	}
	Ok(())
}

pub fn validate_udp_rules(rules: &[UdpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
		match rule.detect.method {
			DetectMethod::Regex => {
				if fancy_regex::Regex::new(&rule.detect.pattern).is_err() {
					let mut err = ValidationError::new("invalid_regex");
					err.message =
						Some(format!("Pattern '{}' is not a valid regex.", rule.detect.pattern).into());
					return Err(err);
				}
			}
			DetectMethod::Fallback => {
				if rule.detect.pattern != "any" {
					let mut err = ValidationError::new("invalid_fallback_pattern");
					err.message = Some("Pattern for 'fallback' method must be 'any'.".into());
					return Err(err);
				}
			}
			_ => {}
		}
	}
	Ok(())
}
