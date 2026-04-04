use std::collections::HashSet;
use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

/// Maximum number of listeners after rule expansion (port ranges + Any protocol).
const MAX_COMPILED_LISTENERS: usize = 10_000;

// -- Types ----------------------------------------------------------------

/// User-facing listener rule with port ranges and protocol shorthand.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct ListenerRule {
	#[serde(default = "default_bind")]
	pub bind: String,
	pub port: String,
	#[serde(default)]
	pub protocol: Protocol,
}

fn default_bind() -> String {
	"0.0.0.0".to_owned()
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
	#[default]
	Tcp,
	Udp,
	Any,
}

impl fmt::Display for Protocol {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Tcp => f.write_str("tcp"),
			Self::Udp => f.write_str("udp"),
			Self::Any => f.write_str("both"),
		}
	}
}

/// A fully resolved listener — one bind address, one port, one protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Type)]
pub struct CompiledListener {
	pub bind: String,
	pub port: u16,
	pub protocol: SingleProtocol,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Type)]
#[serde(rename_all = "lowercase")]
pub enum SingleProtocol {
	Tcp,
	Udp,
}

impl fmt::Display for SingleProtocol {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Tcp => f.write_str("tcp"),
			Self::Udp => f.write_str("udp"),
		}
	}
}

// -- Errors ---------------------------------------------------------------

#[derive(Debug, Error)]
pub enum CompileError {
	#[error("rule #{index}: {message}")]
	RuleError { index: usize, message: String },

	#[error("compiled listener count {count} exceeds limit {MAX_COMPILED_LISTENERS}")]
	TooManyListeners { count: usize },
}

// -- Validation -----------------------------------------------------------

/// Validate a single listener rule. Returns all error messages found.
pub fn validate_rule(rule: &ListenerRule) -> Result<(), Vec<String>> {
	let mut errors = Vec::new();

	if rule.bind.parse::<IpAddr>().is_err() {
		errors.push(format!("bind {:?} is not a valid IP address", rule.bind));
	}

	if let Err(msg) = parse_port_spec(&rule.port) {
		errors.push(msg);
	}

	if errors.is_empty() { Ok(()) } else { Err(errors) }
}

// -- Compilation ----------------------------------------------------------

/// Compile a list of listener rules into fully expanded, deduplicated listeners.
pub fn compile_rules(rules: &[ListenerRule]) -> Result<Vec<CompiledListener>, CompileError> {
	let mut seen = HashSet::new();
	let mut result = Vec::new();

	for (index, rule) in rules.iter().enumerate() {
		let bind: IpAddr = rule.bind.parse().map_err(|_| CompileError::RuleError {
			index,
			message: format!("bind {:?} is not a valid IP address", rule.bind),
		})?;

		let (start, end) =
			parse_port_spec(&rule.port).map_err(|msg| CompileError::RuleError { index, message: msg })?;

		// Port 0 is valid for listeners (OS assigns an ephemeral port)

		let protocols: &[SingleProtocol] = match rule.protocol {
			Protocol::Tcp => &[SingleProtocol::Tcp],
			Protocol::Udp => &[SingleProtocol::Udp],
			Protocol::Any => &[SingleProtocol::Tcp, SingleProtocol::Udp],
		};

		let bind_str = bind.to_string();
		for port in start..=end {
			for &proto in protocols {
				let entry = CompiledListener { bind: bind_str.clone(), port, protocol: proto };
				if seen.insert(entry.clone()) {
					result.push(entry);
				}
			}
		}

		if result.len() > MAX_COMPILED_LISTENERS {
			return Err(CompileError::TooManyListeners { count: result.len() });
		}
	}

	Ok(result)
}

// -- Helpers --------------------------------------------------------------

/// Parse a port spec: either a single port "8080" or a range "8000-8100".
fn parse_port_spec(spec: &str) -> Result<(u16, u16), String> {
	if let Some((left, right)) = spec.split_once('-') {
		let start: u16 =
			left.trim().parse().map_err(|_| format!("port range start {left:?} is not a valid u16"))?;
		let end: u16 =
			right.trim().parse().map_err(|_| format!("port range end {right:?} is not a valid u16"))?;
		if start > end {
			return Err(format!("port range start {start} is greater than end {end}"));
		}
		Ok((start, end))
	} else {
		let port: u16 = spec.trim().parse().map_err(|_| format!("port {spec:?} is not a valid u16"))?;
		Ok((port, port))
	}
}

// -- Tests ----------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;

	fn rule(bind: &str, port: &str, protocol: Protocol) -> ListenerRule {
		ListenerRule { bind: bind.to_owned(), port: port.to_owned(), protocol }
	}

	#[test]
	fn single_port_tcp() {
		let result = compile_rules(&[rule("0.0.0.0", "8080", Protocol::Tcp)]).unwrap();
		assert_eq!(result.len(), 1);
		assert_eq!(result[0].bind, "0.0.0.0");
		assert_eq!(result[0].port, 8080);
		assert_eq!(result[0].protocol, SingleProtocol::Tcp);
	}

	#[test]
	fn port_range_expands() {
		let result = compile_rules(&[rule("0.0.0.0", "8000-8003", Protocol::Tcp)]).unwrap();
		assert_eq!(result.len(), 4);
		let ports: Vec<u16> = result.iter().map(|l| l.port).collect();
		assert_eq!(ports, vec![8000, 8001, 8002, 8003]);
	}

	#[test]
	fn both_expands_to_tcp_and_udp() {
		let result = compile_rules(&[rule("0.0.0.0", "8080", Protocol::Any)]).unwrap();
		assert_eq!(result.len(), 2);
		assert_eq!(result[0].protocol, SingleProtocol::Tcp);
		assert_eq!(result[1].protocol, SingleProtocol::Udp);
	}

	#[test]
	fn both_with_range() {
		let result = compile_rules(&[rule("0.0.0.0", "80-81", Protocol::Any)]).unwrap();
		assert_eq!(result.len(), 4); // 2 ports * 2 protocols
	}

	#[test]
	fn dedup_identical_entries() {
		let rules = vec![
			rule("0.0.0.0", "8080", Protocol::Tcp),
			rule("0.0.0.0", "8080", Protocol::Tcp), // duplicate
		];
		let result = compile_rules(&rules).unwrap();
		assert_eq!(result.len(), 1);
	}

	#[test]
	fn dedup_overlapping_ranges() {
		let rules = vec![
			rule("0.0.0.0", "8000-8002", Protocol::Tcp),
			rule("0.0.0.0", "8001-8003", Protocol::Tcp), // overlaps 8001-8002
		];
		let result = compile_rules(&rules).unwrap();
		assert_eq!(result.len(), 4); // 8000, 8001, 8002, 8003
	}

	#[test]
	fn different_binds_not_deduped() {
		let rules =
			vec![rule("0.0.0.0", "8080", Protocol::Tcp), rule("127.0.0.1", "8080", Protocol::Tcp)];
		let result = compile_rules(&rules).unwrap();
		assert_eq!(result.len(), 2);
	}

	#[test]
	fn ipv6_bind() {
		let result = compile_rules(&[rule("::1", "8080", Protocol::Tcp)]).unwrap();
		assert_eq!(result.len(), 1);
		assert_eq!(result[0].bind, "::1");
	}

	#[test]
	fn invalid_ip() {
		let err = compile_rules(&[rule("not-an-ip", "8080", Protocol::Tcp)]).unwrap_err();
		assert!(err.to_string().contains("not a valid IP"));
	}

	#[test]
	fn invalid_port_string() {
		let err = compile_rules(&[rule("0.0.0.0", "abc", Protocol::Tcp)]).unwrap_err();
		assert!(err.to_string().contains("not a valid u16"));
	}

	#[test]
	fn port_zero_allowed_for_ephemeral() {
		let result = compile_rules(&[rule("0.0.0.0", "0", Protocol::Tcp)]).unwrap();
		assert_eq!(result.len(), 1);
		assert_eq!(result[0].port, 0);
	}

	#[test]
	fn range_reversed() {
		let err = compile_rules(&[rule("0.0.0.0", "9000-8000", Protocol::Tcp)]).unwrap_err();
		assert!(err.to_string().contains("greater than end"));
	}

	#[test]
	fn range_overflow() {
		let err = compile_rules(&[rule("0.0.0.0", "1-99999", Protocol::Tcp)]).unwrap_err();
		assert!(err.to_string().contains("not a valid u16"));
	}

	#[test]
	fn too_many_listeners() {
		// 20000 ports * 2 protocols = 40000 > 10000 limit
		let err = compile_rules(&[rule("0.0.0.0", "1-20000", Protocol::Any)]).unwrap_err();
		assert!(matches!(err, CompileError::TooManyListeners { .. }));
	}

	#[test]
	fn validate_rule_valid() {
		assert!(validate_rule(&rule("0.0.0.0", "8080", Protocol::Tcp)).is_ok());
	}

	#[test]
	fn validate_rule_bad_ip() {
		let errors = validate_rule(&rule("bad", "8080", Protocol::Tcp)).unwrap_err();
		assert!(errors.iter().any(|e| e.contains("not a valid IP")));
	}

	#[test]
	fn validate_rule_bad_port() {
		let errors = validate_rule(&rule("0.0.0.0", "abc", Protocol::Tcp)).unwrap_err();
		assert!(errors.iter().any(|e| e.contains("not a valid u16")));
	}

	#[test]
	fn validate_rule_zero_port_allowed() {
		assert!(validate_rule(&rule("0.0.0.0", "0", Protocol::Tcp)).is_ok());
	}

	#[test]
	fn validate_rule_multiple_errors() {
		let errors = validate_rule(&rule("bad", "abc", Protocol::Tcp)).unwrap_err();
		assert!(errors.len() >= 2);
	}

	#[test]
	fn empty_rules_compiles_to_empty() {
		let result = compile_rules(&[]).unwrap();
		assert!(result.is_empty());
	}

	#[test]
	fn single_port_range_same_as_single() {
		let result = compile_rules(&[rule("0.0.0.0", "8080-8080", Protocol::Tcp)]).unwrap();
		assert_eq!(result.len(), 1);
		assert_eq!(result[0].port, 8080);
	}

	#[test]
	fn udp_only() {
		let result = compile_rules(&[rule("0.0.0.0", "53", Protocol::Udp)]).unwrap();
		assert_eq!(result.len(), 1);
		assert_eq!(result[0].protocol, SingleProtocol::Udp);
	}

	#[test]
	fn serde_roundtrip() {
		let r = rule("127.0.0.1", "8080-8082", Protocol::Any);
		let json = serde_json::to_string(&r).unwrap();
		let back: ListenerRule = serde_json::from_str(&json).unwrap();
		assert_eq!(r, back);
	}

	#[test]
	fn compiled_listener_serde_roundtrip() {
		let c =
			CompiledListener { bind: "0.0.0.0".to_owned(), port: 8080, protocol: SingleProtocol::Tcp };
		let json = serde_json::to_string(&c).unwrap();
		let back: CompiledListener = serde_json::from_str(&json).unwrap();
		assert_eq!(c, back);
	}
}
