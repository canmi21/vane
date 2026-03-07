use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};

/// A target endpoint in configuration, either IP-based or domain-based.
///
/// ```
/// use vane_primitives::model::Target;
///
/// let ip_target: Target = serde_json::from_str(r#"{"ip":"10.0.0.1","port":443}"#).unwrap();
/// assert!(matches!(ip_target, Target::Ip { .. }));
///
/// let domain_target: Target =
///     serde_json::from_str(r#"{"domain":"example.com","port":8080}"#).unwrap();
/// assert!(matches!(domain_target, Target::Domain { .. }));
///
/// // Roundtrip
/// let json = serde_json::to_string(&ip_target).unwrap();
/// let back: Target = serde_json::from_str(&json).unwrap();
/// assert_eq!(ip_target, back);
/// ```
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum Target {
	Ip { ip: IpAddr, port: u16 },
	Domain { domain: String, port: u16 },
}

/// A fully resolved target with a concrete socket address.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedTarget {
	pub addr: SocketAddr,
}

/// Load-balancing strategy for forwarding.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
	Random,
	Serial,
	Fastest,
}

/// Forwarding configuration with strategy and target lists.
///
/// ```
/// use vane_primitives::model::{Forward, Strategy};
///
/// let forward: Forward = serde_json::from_str(r#"{
///     "strategy": "random",
///     "targets": [{"ip": "10.0.0.1", "port": 443}]
/// }"#).unwrap();
///
/// assert_eq!(forward.strategy, Strategy::Random);
/// assert_eq!(forward.targets.len(), 1);
/// assert!(forward.fallbacks.is_empty()); // defaults to empty
/// ```
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Forward {
	pub strategy: Strategy,
	#[serde(default)]
	pub targets: Vec<Target>,
	#[serde(default)]
	pub fallbacks: Vec<Target>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;

	#[test]
	fn target_ip_serde_roundtrip() {
		let target = Target::Ip { ip: "10.0.0.1".parse().unwrap(), port: 443 };
		let json = serde_json::to_string(&target).unwrap();
		let back: Target = serde_json::from_str(&json).unwrap();
		assert_eq!(target, back);
	}

	#[test]
	fn target_domain_serde_roundtrip() {
		let target = Target::Domain { domain: "example.com".to_owned(), port: 8080 };
		let json = serde_json::to_string(&target).unwrap();
		let back: Target = serde_json::from_str(&json).unwrap();
		assert_eq!(target, back);
	}

	#[test]
	fn untagged_deserialize_distinguishes_variants() {
		let ip_json = r#"{"ip":"127.0.0.1","port":80}"#;
		let domain_json = r#"{"domain":"example.com","port":443}"#;

		let ip: Target = serde_json::from_str(ip_json).unwrap();
		let domain: Target = serde_json::from_str(domain_json).unwrap();

		assert!(matches!(ip, Target::Ip { .. }));
		assert!(matches!(domain, Target::Domain { .. }));
	}

	#[test]
	fn strategy_snake_case_serialization() {
		assert_eq!(serde_json::to_string(&Strategy::Random).unwrap(), r#""random""#);
		assert_eq!(serde_json::to_string(&Strategy::Serial).unwrap(), r#""serial""#);
		assert_eq!(serde_json::to_string(&Strategy::Fastest).unwrap(), r#""fastest""#);
	}

	#[test]
	fn forward_default_empty_vecs() {
		let json = r#"{"strategy":"random"}"#;
		let forward: Forward = serde_json::from_str(json).unwrap();
		assert_eq!(forward.strategy, Strategy::Random);
		assert!(forward.targets.is_empty());
		assert!(forward.fallbacks.is_empty());
	}

	#[test]
	fn forward_with_targets() {
		let json = r#"{
            "strategy": "serial",
            "targets": [{"ip": "10.0.0.1", "port": 443}],
            "fallbacks": [{"domain": "backup.example.com", "port": 8080}]
        }"#;
		let forward: Forward = serde_json::from_str(json).unwrap();
		assert_eq!(forward.strategy, Strategy::Serial);
		assert_eq!(forward.targets.len(), 1);
		assert_eq!(forward.fallbacks.len(), 1);
	}

	#[test]
	fn resolved_target_from_socket_addr() {
		let addr: SocketAddr = "10.0.0.1:8080".parse().unwrap();
		let target = ResolvedTarget { addr };
		assert_eq!(target.addr, addr);
	}

	#[test]
	fn target_ipv6_serde_roundtrip() {
		let target = Target::Ip { ip: "::1".parse().unwrap(), port: 443 };
		let json = serde_json::to_string(&target).unwrap();
		let back: Target = serde_json::from_str(&json).unwrap();
		assert_eq!(target, back);
	}

	#[test]
	fn invalid_target_json_error() {
		let bad_json = r#"{"not_ip_or_domain": true}"#;
		let result = serde_json::from_str::<Target>(bad_json);
		assert!(result.is_err());
	}
}
