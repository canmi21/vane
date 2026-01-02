/* src/modules/plugins/terminators/transport/proxy/mod.rs */

pub mod domain;
pub mod ip;
pub mod node;
pub mod proxy;

use crate::modules::{
	kv::KvStore, plugins::core::model::ConnectionObject, stack::transport::model::ResolvedTarget,
};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};

pub async fn execute_proxy(
	target: ResolvedTarget,
	kv: &KvStore,
	conn: ConnectionObject,
) -> Result<()> {
	let protocol = kv
		.get("conn.proto")
		.map(|s| s.as_str())
		.unwrap_or("unknown");

	match conn {
		ConnectionObject::Tcp(stream) => {
			proxy::proxy_tcp_stream(stream, target).await?;
		}
		ConnectionObject::Stream(stream) => {
			log(
				LogLevel::Debug,
				&format!(
					"➜ Proxying L4+ Stream ({}) to {}:{}",
					protocol, target.ip, target.port
				),
			);
			proxy::proxy_generic_stream(stream, target).await?;
		}
		ConnectionObject::Udp {
			socket,
			datagram,
			client_addr,
		} => {
			let is_quic = kv
				.get("conn.proto.carrier")
				.map(|p| p == "quic")
				.unwrap_or(false);
			if is_quic {
				proxy::proxy_quic_association(socket, &datagram, client_addr, target).await?;
			} else {
				proxy::proxy_udp_direct(socket, &datagram, client_addr, target).await?;
			}
		}
		ConnectionObject::Virtual(desc) => {
			return Err(anyhow!(
				"Cannot transport-proxy a Virtual connection: {}",
				desc
			));
		}
	}

	Ok(())
}
