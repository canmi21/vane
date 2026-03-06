use std::fmt;
use std::net::SocketAddr;

use thiserror::Error;

#[derive(Debug, Clone, Copy)]
pub enum TransferDirection {
	ClientToServer,
	ServerToClient,
}

impl fmt::Display for TransferDirection {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::ClientToServer => f.write_str("client->server"),
			Self::ServerToClient => f.write_str("server->client"),
		}
	}
}

#[derive(Debug, Error)]
pub enum ProxyError {
	#[error("failed to connect to {addr}")]
	ConnectFailed { addr: SocketAddr, source: std::io::Error },

	#[error("connect timeout after {timeout_secs}s to {addr}")]
	ConnectTimeout { addr: SocketAddr, timeout_secs: u64 },

	#[error("idle timeout after {idle_secs}s")]
	IdleTimeout { idle_secs: u64 },

	#[error("transfer failed ({direction})")]
	TransferFailed { direction: TransferDirection, source: std::io::Error },
}

#[derive(Debug, Error)]
pub enum ListenerError {
	#[error("failed to bind {addr} after {attempts} attempts")]
	BindFailed { addr: SocketAddr, attempts: u32, source: std::io::Error },
}
