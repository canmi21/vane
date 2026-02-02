/* src/layers/l4/resolver.rs */

use super::model::{ResolvedTarget, Target};
use fancy_log::{LogLevel, log};
#[cfg(feature = "domain-target")]
use hickory_resolver::{
	TokioResolver,
	config::{NameServerConfig, ResolverConfig, ResolverOpts},
	name_server::TokioConnectionProvider,
	proto::xfer::Protocol,
};
use once_cell::sync::Lazy;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;

#[cfg(feature = "domain-target")]
static DNS_RESOLVER: Lazy<TokioResolver> = Lazy::new(|| {
	let ns1_str = envflag::get_string("NAMESERVER1", "1.1.1.1");
	let ns1_port = envflag::get::<u16>("NAMESERVER1_PORT", 53);
	let ns2_str = envflag::get_string("NAMESERVER2", "8.8.8.8");
	let ns2_port = envflag::get::<u16>("NAMESERVER2_PORT", 53);

	let mut config = ResolverConfig::new();

	if let Ok(ip1) = Ipv4Addr::from_str(&ns1_str) {
		let sock_addr = SocketAddr::new(IpAddr::V4(ip1), ns1_port);
		config.add_name_server(NameServerConfig::new(sock_addr, Protocol::Udp));
	} else {
		log(
			LogLevel::Warn,
			"✗ Invalid format for NAMESERVER1 or NAMESERVER1_PORT",
		);
	}

	if let Ok(ip2) = Ipv4Addr::from_str(&ns2_str) {
		let sock_addr = SocketAddr::new(IpAddr::V4(ip2), ns2_port);
		config.add_name_server(NameServerConfig::new(sock_addr, Protocol::Udp));
	} else {
		log(
			LogLevel::Warn,
			"✗ Invalid format for NAMESERVER2 or NAMESERVER2_PORT",
		);
	}

	TokioResolver::builder_with_config(config, TokioConnectionProvider::default())
		.with_options(ResolverOpts::default())
		.build()
});

#[cfg(feature = "domain-target")]
pub async fn resolve_domain_to_ips(domain: &str) -> Vec<IpAddr> {
	log(LogLevel::Debug, &format!("⚙ Resolving domain: {domain}"));
	match DNS_RESOLVER.lookup_ip(domain).await {
		Ok(lookup) => lookup.iter().collect(),
		Err(e) => {
			log(
				LogLevel::Warn,
				&format!("✗ DNS lookup failed for {domain}: {e}"),
			);
			Vec::new()
		}
	}
}

pub async fn resolve_targets(targets: &[Target]) -> Vec<ResolvedTarget> {
	let mut resolved = Vec::new();
	let config_manager = crate::config::get();
	let nodes_config = config_manager
		.nodes
		.get()
		.unwrap_or_else(|| Arc::new(crate::config::NodesConfig::default()));

	for target in targets {
		match target {
			Target::Ip { ip, port } => {
				resolved.push(ResolvedTarget {
					ip: ip.clone(),
					port: *port,
				});
			}
			#[cfg(feature = "domain-target")]
			Target::Domain { domain, port } => {
				let ips = resolve_domain_to_ips(domain).await;
				for ip in ips {
					resolved.push(ResolvedTarget {
						ip: ip.to_string(),
						port: *port,
					});
				}
			}
			#[cfg(not(feature = "domain-target"))]
			Target::Domain { domain, .. } => {
				log(
					LogLevel::Error,
					&format!(
						"✗ Domain target '{}' ignored because 'domain-target' feature is disabled.",
						domain
					),
				);
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
						&format!("✗ Node '{node}' with port {port} not found in nodes configuration."),
					);
				}
			}
		}
	}
	resolved
}
