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
