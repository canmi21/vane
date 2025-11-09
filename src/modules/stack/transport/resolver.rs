/* src/modules/stack/transport/resolver.rs */

use super::model::{ResolvedTarget, Target};
use crate::common::getenv;
use crate::modules::nodes::model::NODES_STATE;
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use trust_dns_resolver::{
	TokioAsyncResolver,
	config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts},
};

static DNS_RESOLVER: Lazy<TokioAsyncResolver> = Lazy::new(|| {
	let ns1_str = getenv::get_env("NAMESERVER1", "1.1.1.1".to_string());
	let ns1_port_str = getenv::get_env("NAMESERVER1_PORT", "53".to_string());
	let ns2_str = getenv::get_env("NAMESERVER2", "8.8.8.8".to_string());
	let ns2_port_str = getenv::get_env("NAMESERVER2_PORT", "53".to_string());

	let mut config = ResolverConfig::new();

	if let (Ok(ip1), Ok(port1)) = (Ipv4Addr::from_str(&ns1_str), ns1_port_str.parse::<u16>()) {
		let sock_addr = SocketAddr::new(IpAddr::V4(ip1), port1);
		config.add_name_server(NameServerConfig::new(sock_addr, Protocol::Udp));
	} else {
		log(
			LogLevel::Warn,
			&format!("✗ Invalid format for NAMESERVER1 or NAMESERVER1_PORT"),
		);
	}

	if let (Ok(ip2), Ok(port2)) = (Ipv4Addr::from_str(&ns2_str), ns2_port_str.parse::<u16>()) {
		let sock_addr = SocketAddr::new(IpAddr::V4(ip2), port2);
		config.add_name_server(NameServerConfig::new(sock_addr, Protocol::Udp));
	} else {
		log(
			LogLevel::Warn,
			&format!("✗ Invalid format for NAMESERVER2 or NAMESERVER2_PORT"),
		);
	}

	TokioAsyncResolver::tokio(config, ResolverOpts::default())
});

/// Resolves a list of abstract Targets into a flat list of concrete ResolvedTargets.
pub async fn resolve_targets(targets: &[Target]) -> Vec<ResolvedTarget> {
	let mut resolved = Vec::new();
	let nodes_config = NODES_STATE.load();

	for target in targets {
		match target {
			Target::Ip { ip, port } => {
				resolved.push(ResolvedTarget {
					ip: ip.clone(),
					port: *port,
				});
			}
			Target::Domain { domain, port } => {
				log(LogLevel::Debug, &format!("⚙ Resolving domain: {}", domain));
				match DNS_RESOLVER.lookup_ip(domain.as_str()).await {
					Ok(lookup) => {
						for ip in lookup.iter() {
							resolved.push(ResolvedTarget {
								ip: ip.to_string(),
								port: *port,
							});
						}
					}
					Err(e) => log(
						LogLevel::Warn,
						&format!("✗ DNS lookup failed for {}: {}", domain, e),
					),
				}
			}
			Target::Node { node, port } => {
				let mut found = false;
				for p_node in &nodes_config.processed {
					if &p_node.node_name == node && p_node.port == *port {
						resolved.push(ResolvedTarget {
							ip: p_node.address.clone(),
							port: *port,
						});
						found = true;
					}
				}
				if !found {
					log(
						LogLevel::Warn,
						&format!(
							"✗ Node '{}' with port {} not found in nodes configuration.",
							node, port
						),
					);
				}
			}
		}
	}
	resolved
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::nodes::model::{IpType, NodesConfig, ProcessedNode};
	use serial_test::serial;
	use std::sync::Arc;

	/// Cleans up the NODES_STATE global after a test by storing a default, empty config.
	fn cleanup_globals() {
		NODES_STATE.store(Arc::new(NodesConfig::default()));
	}

	/// Tests that a simple Ip target is resolved correctly (pass-through).
	#[tokio::test]
	#[serial]
	async fn test_resolve_ip_target() {
		cleanup_globals();
		let targets = vec![Target::Ip {
			ip: "192.168.1.1".to_string(),
			port: 8080,
		}];
		let resolved = resolve_targets(&targets).await;

		assert_eq!(resolved.len(), 1);
		assert_eq!(
			resolved[0],
			ResolvedTarget {
				ip: "192.168.1.1".to_string(),
				port: 8080
			}
		);
	}

	/// Tests that a Node target is correctly resolved from the global NODES_STATE.
	#[tokio::test]
	#[serial]
	async fn test_resolve_node_target() {
		// 1. Setup: Create a mock nodes configuration and load it into the global state.
		let mock_nodes_config = NodesConfig {
			processed: vec![
				ProcessedNode {
					node_name: "cache-redis".to_string(),
					address: "10.0.1.5".to_string(),
					port: 6379,
					ip_type: IpType::Ipv4,
				},
				ProcessedNode {
					node_name: "database".to_string(),
					address: "10.0.2.10".to_string(),
					port: 5432,
					ip_type: IpType::Ipv4,
				},
			],
			..Default::default()
		};
		NODES_STATE.store(Arc::new(mock_nodes_config));

		// 2. Define targets, including one that exists and one that doesn't.
		let targets = vec![
			Target::Node {
				node: "cache-redis".to_string(),
				port: 6379,
			},
			Target::Node {
				node: "non-existent-node".to_string(),
				port: 1234,
			},
		];

		// 3. Act: Resolve the targets.
		let resolved = resolve_targets(&targets).await;

		// 4. Assert: Verify that only the valid node was resolved.
		assert_eq!(
			resolved.len(),
			1,
			"Should only resolve the node that exists in the state"
		);
		assert_eq!(
			resolved[0],
			ResolvedTarget {
				ip: "10.0.1.5".to_string(),
				port: 6379
			}
		);

		cleanup_globals();
	}

	/// Tests that a Domain target ('localhost') is resolved correctly.
	#[tokio::test]
	#[serial]
	async fn test_resolve_domain_target_localhost() {
		cleanup_globals();
		let targets = vec![Target::Domain {
			domain: "localhost".to_string(),
			port: 9000,
		}];
		let resolved = resolve_targets(&targets).await;

		assert!(
			!resolved.is_empty(),
			"Resolving 'localhost' should yield at least one IP (v4 or v6)"
		);
		// Check that one of the common localhost IPs is present.
		let has_localhost_ip = resolved
			.iter()
			.any(|rt| rt.ip == "127.0.0.1" || rt.ip == "::1");
		assert!(
			has_localhost_ip,
			"Resolved list should contain a standard localhost IP"
		);
		assert_eq!(
			resolved[0].port, 9000,
			"Port should be correctly carried over"
		);
	}

	/// Tests resolving a mix of all target types in a single call.
	#[tokio::test]
	#[serial]
	async fn test_resolve_mixed_targets() {
		// 1. Setup Node state
		let mock_nodes_config = NodesConfig {
			processed: vec![ProcessedNode {
				node_name: "api".to_string(),
				address: "172.16.0.100".to_string(),
				port: 80,
				ip_type: IpType::Ipv4, // CORRECTED: Used the correct enum variant
			}],
			..Default::default()
		};
		NODES_STATE.store(Arc::new(mock_nodes_config));

		// 2. Define mixed targets
		let targets = vec![
			Target::Ip {
				ip: "8.8.8.8".to_string(),
				port: 53,
			},
			Target::Node {
				node: "api".to_string(),
				port: 80,
			},
		];

		// 3. Act
		let resolved = resolve_targets(&targets).await;

		// 4. Assert
		assert_eq!(resolved.len(), 2, "Should resolve both IP and Node targets");
		assert!(resolved.contains(&ResolvedTarget {
			ip: "8.8.8.8".to_string(),
			port: 53
		}));
		assert!(resolved.contains(&ResolvedTarget {
			ip: "172.16.0.100".to_string(),
			port: 80
		}));

		cleanup_globals();
	}
}
