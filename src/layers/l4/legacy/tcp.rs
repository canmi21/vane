/* src/layers/l4/legacy/tcp.rs */

use crate::common::config::env_loader;
use crate::layers::l4::model::{Detect, DetectMethod, Forward};
use crate::layers::l4::{balancer, proxy};
use crate::resources::kv::KvStore;
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::{io::AsyncWriteExt, net::TcpStream};
use validator::{Validate, ValidationError, ValidationErrors};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TcpSession {
	pub keepalive: bool,
	pub timeout: u64,
}

impl Validate for TcpSession {
	fn validate(&self) -> Result<(), ValidationErrors> {
		if self.timeout == 0 {
			let mut errors = ValidationErrors::new();
			let mut err = ValidationError::new("range");
			err.message = Some("timeout must be greater than 0".into());
			errors.add("timeout", err);
			return Err(errors);
		}
		Ok(())
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TcpDestination {
	Resolver { resolver: String },
	Forward { forward: Forward },
}

impl Validate for TcpDestination {
	fn validate(&self) -> Result<(), ValidationErrors> {
		match self {
			TcpDestination::Resolver { .. } => Ok(()),
			TcpDestination::Forward { forward } => forward.validate(),
		}
	}
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq)]
pub struct TcpProtocolRule {
	#[validate(regex(
        path = *crate::layers::l4::model::NAME_REGEX,
        message = "can only contain lowercase letters, numbers, underscores and hyphens"
    ))]
	pub name: String,
	#[validate(range(min = 1))]
	pub priority: u32,
	#[validate(nested)]
	pub detect: Detect,
	#[serde(default)]
	#[validate(nested)]
	pub session: Option<TcpSession>,
	#[validate(nested)]
	pub destination: TcpDestination,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Validate)]
pub struct LegacyTcpConfig {
	#[serde(rename = "protocols")]
	#[validate(nested)]
	pub rules: Vec<TcpProtocolRule>,
}

pub async fn dispatch_legacy_tcp(
	mut socket: TcpStream,
	port: u16,
	config: &LegacyTcpConfig,
	_kv_store: KvStore,
) {
	let peer_addr = socket
		.peer_addr()
		.map_or_else(|_| "unknown".to_string(), |a| a.to_string());
	let mut rules = config.rules.clone();
	rules.sort_by_key(|r| r.priority);

	let limit_str = env_loader::get_env("TCP_DETECT_LIMIT", "64".to_string());
	let limit = limit_str.parse::<usize>().unwrap_or(64);
	const MAX_DETECT_LIMIT: usize = 8192;
	let final_limit = limit.min(MAX_DETECT_LIMIT);
	let mut buf = vec![0u8; final_limit];

	let n = match socket.peek(&mut buf).await {
		Ok(n) => n,
		Err(e) => {
			log(
				LogLevel::Warn,
				&format!("⚠ Failed to peek initial data from {}: {}", peer_addr, e),
			);
			return;
		}
	};

	if n == 0 {
		log(LogLevel::Debug, "⚙ Connection closed.");
		return;
	}

	let incoming_data = &buf[..n];
	for rule in rules {
		let matches = match &rule.detect.method {
			DetectMethod::Magic => {
				if let Some(hex_str) = rule.detect.pattern.strip_prefix("0x") {
					u8::from_str_radix(hex_str, 16).map_or(false, |b| incoming_data.starts_with(&[b]))
				} else {
					false
				}
			}
			DetectMethod::Prefix => {
				let pattern_bytes = rule.detect.pattern.as_bytes();
				incoming_data
					.windows(pattern_bytes.len())
					.any(|window| window == pattern_bytes)
			}
			DetectMethod::Regex => {
				#[cfg(any(feature = "tcp", feature = "udp"))]
				{
					if let Ok(re) = fancy_regex::Regex::new(&rule.detect.pattern) {
						if let Ok(data_str) = std::str::from_utf8(incoming_data) {
							re.is_match(data_str).unwrap_or(false)
						} else {
							false
						}
					} else {
						false
					}
				}
				#[cfg(not(any(feature = "tcp", feature = "udp")))]
				false
			}
			DetectMethod::Fallback => true,
		};
		if matches {
			log(
				LogLevel::Info,
				&format!(
					"➜ Matched Protocol[{}] {} for connection from {}",
					rule.priority, rule.name, peer_addr
				),
			);
			match rule.destination {
				TcpDestination::Resolver { resolver } => {
					log(LogLevel::Debug, &format!("⚙ Legacy Resolver: {}", resolver));
					// legacy resolver placeholder
					return;
				}
				TcpDestination::Forward { ref forward } => {
					if let Some(target) = balancer::select_tcp_target(port, &rule.name, forward).await {
						let _ = proxy::proxy_tcp_stream(socket, target).await;
					} else {
						log(LogLevel::Warn, "⚠ No available targets.");
					}
					return;
				}
			}
		}
	}
	let _ = socket.shutdown().await;
}

pub fn validate_tcp_rules(rules: &[TcpProtocolRule]) -> Result<(), ValidationError> {
	let mut priorities = HashSet::new();
	for rule in rules {
		if !priorities.insert(rule.priority) {
			let mut err = ValidationError::new("unique_priorities");
			err.message = Some("Priorities must be unique within a listener config.".into());
			return Err(err);
		}
	}
	Ok(())
}
