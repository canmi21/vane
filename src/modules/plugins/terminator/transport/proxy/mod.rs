/* src/modules/plugins/terminator/transport/proxy/mod.rs */

pub mod domain;
pub mod ip;
pub mod node;
pub mod proxy;

use crate::modules::{
	kv::KvStore, plugins::model::ConnectionObject, stack::transport::model::ResolvedTarget,
};
use anyhow::Result;
use fancy_log::{LogLevel, log};

/// Common execution logic for transport proxy plugins.
/// Acts as a polymorphic dispatcher based on the ConnectionObject type.
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
		// Case 1: Raw TCP (L4)
		ConnectionObject::Tcp(stream) => {
			proxy::proxy_tcp_stream(stream, target).await?;
		}

		// Case 2: Generic/Encrypted Stream (L4+)
		ConnectionObject::Stream(stream) => {
			log(
				LogLevel::Debug,
				&format!(
					"➜ Proxying L4+ Stream ({}) to upstream {}:{}",
					protocol, target.ip, target.port
				),
			);
			proxy::proxy_generic_stream(stream, target).await?;
		}

		// Case 3: UDP Datagram
		ConnectionObject::Udp {
			socket,
			datagram,
			client_addr,
		} => {
			log(
				LogLevel::Debug,
				&format!(
					"➜ Proxying UDP datagram from {} to {}:{}",
					client_addr, target.ip, target.port
				),
			);
			proxy::proxy_udp_direct(socket, &datagram, client_addr, target).await?;

			log(
				LogLevel::Debug,
				&format!("✓ UDP proxy action initiated for {}.", client_addr),
			);
		}
	}

	Ok(())
}
