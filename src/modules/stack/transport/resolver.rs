/* src/modules/stack/transport/resolver.rs */

use super::model::{ResolvedTarget, Target};
use crate::common::getenv;
use crate::modules::nodes::model::NODES_STATE;
use fancy_log::{LogLevel, log};
use hickory_resolver::{
	TokioResolver,
	config::{NameServerConfig, ResolverConfig, ResolverOpts},
	name_server::TokioConnectionProvider,
	proto::xfer::Protocol,
};
use once_cell::sync::Lazy;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;

static DNS_RESOLVER: Lazy<TokioResolver> = Lazy::new(|| {
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

	TokioResolver::builder_with_config(config, TokioConnectionProvider::default())
		.with_options(ResolverOpts::default())
		.build()
});

pub async fn resolve_domain_to_ips(domain: &str) -> Vec<IpAddr> {
	log(LogLevel::Debug, &format!("⚙ Resolving domain: {}", domain));
	match DNS_RESOLVER.lookup_ip(domain).await {
		Ok(lookup) => lookup.iter().collect(),
		Err(e) => {
			log(
				LogLevel::Warn,
				&format!("✗ DNS lookup failed for {}: {}", domain, e),
			);
			Vec::new()
		}
	}
}

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
				let ips = resolve_domain_to_ips(domain).await;
				for ip in ips {
					resolved.push(ResolvedTarget {
						ip: ip.to_string(),
						port: *port,
					});
				}
			}
			Target::Node { node, port } => {
				let mut found = false;
				if let Some(found_node) = nodes_config.nodes.iter().find(|n| &n.name == node) {
					for ip_config in &found_node.ips {
						if ip_config.ports.contains(port) {
							resolved.push(ResolvedTarget {
								ip: ip_config.address.clone(),
								port: *port,
							});
							found = true;
						}
					}
				}

				if !found {
					log(
						LogLevel::Debug,
						&format!(
							"⚙ Node lookup failed. Current nodes state being searched: {:?}",
							nodes_config.nodes
						),
					);
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
