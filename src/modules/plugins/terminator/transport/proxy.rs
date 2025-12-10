/* src/modules/plugins/terminator/transport/proxy.rs */

use crate::modules::{
	kv::KvStore,
	plugins::model::ConnectionObject,
	stack::transport::{model::ResolvedTarget, proxy as stack_proxy},
};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};

/// Common execution logic for transport proxy plugins.
/// Takes a resolved target (IP + Port) and bridges the connection based on the protocol.
pub async fn execute_proxy(
	target: ResolvedTarget,
	kv: &KvStore,
	conn: ConnectionObject,
) -> Result<()> {
	let protocol = kv
		.get("conn.proto")
		.map(|s| s.as_str())
		.unwrap_or("unknown");

	match (protocol, conn) {
		("tcp", ConnectionObject::Tcp(stream)) => {
			stack_proxy::proxy_tcp_stream(stream, target).await?;
		}
		(
			"udp",
			ConnectionObject::Udp {
				socket,
				datagram,
				client_addr,
			},
		) => {
			log(
				LogLevel::Debug,
				&format!(
					"➜ Proxying UDP datagram from {} to {}:{}",
					client_addr, target.ip, target.port
				),
			);
			stack_proxy::proxy_udp_direct(socket, &datagram, client_addr, target).await?;

			log(
				LogLevel::Debug,
				&format!("✓ UDP proxy action initiated for {}.", client_addr),
			);
		}
		(proto, _) => {
			return Err(anyhow!(
				"Protocol mismatch: KvStore says '{}', but received a different ConnectionObject type.",
				proto
			));
		}
	}

	Ok(())
}
