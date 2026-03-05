/* src/resources/service_discovery/model.rs */

use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashSet;
#[cfg(feature = "console")]
use utoipa::ToSchema;
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

lazy_static! {
	static ref NAME_REGEX: regex::Regex =
		regex::Regex::new(r"^[a-z0-9-]+$").expect("Failed to compile NAME_REGEX");
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub enum IpType {
	Ipv4,
	Ipv6,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub struct IpConfig {
	#[validate(ip)]
	pub address: String,
	#[validate(length(min = 1, message = "must have at least one port"))]
	#[serde(default)]
	pub ports: Vec<u16>,
	pub r#type: IpType,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "console", derive(ToSchema))]
pub struct Node {
	#[validate(regex(path = *NAME_REGEX, message = "can only contain lowercase letters, numbers, and hyphens"))]
	pub name: String,
	#[validate(length(min = 1, message = "must have at least one IP configuration"))]
	#[validate(nested)]
	#[serde(default)]
	pub ips: Vec<IpConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProcessedNode {
	pub node_name: String,
	pub address: String,
	pub port: u16,
	pub ip_type: IpType,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct NodesConfig {
	#[serde(default)]
	pub nodes: Vec<Node>,
	#[serde(skip)]
	pub processed: Vec<ProcessedNode>,
}

fn validate_unique_node_names(nodes: &[Node]) -> Result<(), ValidationError> {
	let mut names = HashSet::new();
	for node in nodes {
		if !names.insert(&node.name) {
			let mut err = ValidationError::new("unique_node_names");
			err.message = Some(format!("Node name '{}' is not unique.", node.name).into());
			return Err(err);
		}
	}
	Ok(())
}

impl Validate for NodesConfig {
	fn validate(&self) -> Result<(), ValidationErrors> {
		let mut validation_errors = ValidationErrors::new();

		for (i, node) in self.nodes.iter().enumerate() {
			if let Err(node_errors) = node.validate() {
				for (field, kind) in node_errors.errors() {
					if let ValidationErrorsKind::Field(field_errors) = kind {
						for error in field_errors {
							let mut err = error.clone();
							let old_msg = err.message.clone().unwrap_or_else(|| Cow::from("invalid"));
							err.message = Some(format!("[node {i}] {field}: {old_msg}").into());
							validation_errors.add("nodes", err);
						}
					}
				}
			}
		}

		if let Err(e) = validate_unique_node_names(&self.nodes) {
			validation_errors.add("nodes", e);
		}

		if validation_errors.is_empty() { Ok(()) } else { Err(validation_errors) }
	}
}

impl live::loader::PreProcess for NodesConfig {
	fn pre_process(&mut self) {
		let mut processed_list = Vec::new();
		for node in &self.nodes {
			for ip_config in &node.ips {
				for &port in &ip_config.ports {
					processed_list.push(ProcessedNode {
						node_name: node.name.clone(),
						address: ip_config.address.clone(),
						port,
						ip_type: ip_config.r#type.clone(),
					});
				}
			}
		}
		self.processed = processed_list;
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use live::loader::PreProcess;

	// --- Test Helpers to create valid default structs ---

	fn valid_ip_config_v4() -> IpConfig {
		IpConfig { address: "192.168.1.1".to_string(), ports: vec![80, 443], r#type: IpType::Ipv4 }
	}

	fn valid_ip_config_v6() -> IpConfig {
		IpConfig { address: "2001:db8::1".to_string(), ports: vec![8080], r#type: IpType::Ipv6 }
	}

	fn valid_node() -> Node {
		Node { name: "my-web-server".to_string(), ips: vec![valid_ip_config_v4()] }
	}

	/// Tests the validation logic for the IpConfig struct.
	#[test]
	fn test_ip_config_validation() {
		// Valid config should pass.
		let mut config = valid_ip_config_v4();
		assert!(config.validate().is_ok());

		// Invalid IP address should fail.
		config.address = "not-an-ip".to_string();
		assert!(config.validate().is_err());
		config.address = "192.168.1.1".to_string(); // Reset

		// Empty ports list should fail.
		config.ports = vec![];
		assert!(config.validate().is_err());
	}

	/// Tests the validation logic for the Node struct.
	#[test]
	fn test_node_validation() {
		// Valid node should pass.
		let mut node = valid_node();
		assert!(node.validate().is_ok());

		// Invalid name (uppercase) should fail.
		node.name = "MyWebServer".to_string();
		assert!(node.validate().is_err());
		node.name = "my-web-server".to_string(); // Reset

		// Empty ips list should fail.
		node.ips = vec![];
		assert!(node.validate().is_err());
		node.ips = vec![valid_ip_config_v4()]; // Reset

		// Nested validation: an invalid IpConfig should make the Node invalid.
		node.ips[0].address = "invalid".to_string();
		assert!(node.validate().is_err());
	}

	/// Tests the validation logic for the top-level NodesConfig struct.
	#[test]
	fn test_nodes_config_validation() {
		// Valid config with multiple unique nodes should pass.
		let mut config = NodesConfig {
			nodes: vec![
				valid_node(),
				Node { name: "my-db-server".to_string(), ips: vec![valid_ip_config_v6()] },
			],
			..Default::default()
		};
		assert!(config.validate().is_ok());

		// Duplicate node names should fail.
		config.nodes[1].name = "my-web-server".to_string();
		assert!(config.validate().is_err());
		config.nodes[1].name = "my-db-server".to_string(); // Reset

		// Nested validation: an invalid Node should make the NodesConfig invalid.
		config.nodes[0].name = "INVALID_NAME".to_string();
		assert!(config.validate().is_err());
	}

	/// Tests the pre-processing logic that populates the `processed` field.
	#[test]
	fn test_nodes_config_pre_process() {
		let mut config = NodesConfig {
			nodes: vec![
				Node {
					name: "node-a".to_string(),
					ips: vec![
						IpConfig { address: "10.0.0.1".to_string(), ports: vec![80, 81], r#type: IpType::Ipv4 },
						IpConfig { address: "10.0.0.2".to_string(), ports: vec![90], r#type: IpType::Ipv4 },
					],
				},
				Node {
					name: "node-b".to_string(),
					ips: vec![IpConfig {
						address: "::1".to_string(),
						ports: vec![100],
						r#type: IpType::Ipv6,
					}],
				},
			],
			..Default::default()
		};

		// The `processed` list should be empty initially.
		assert!(config.processed.is_empty());

		// Run the pre-processing.
		config.pre_process();

		// The `processed` list should now be populated.
		// (2 ports from node-a's first IP) + (1 port from node-a's second IP) + (1 port from node-b) = 4 total
		assert_eq!(config.processed.len(), 4);

		// Verify the contents of the processed list.
		let expected_processed = vec![
			ProcessedNode {
				node_name: "node-a".to_string(),
				address: "10.0.0.1".to_string(),
				port: 80,
				ip_type: IpType::Ipv4,
			},
			ProcessedNode {
				node_name: "node-a".to_string(),
				address: "10.0.0.1".to_string(),
				port: 81,
				ip_type: IpType::Ipv4,
			},
			ProcessedNode {
				node_name: "node-a".to_string(),
				address: "10.0.0.2".to_string(),
				port: 90,
				ip_type: IpType::Ipv4,
			},
			ProcessedNode {
				node_name: "node-b".to_string(),
				address: "::1".to_string(),
				port: 100,
				ip_type: IpType::Ipv6,
			},
		];

		// The order is deterministic, so we can compare the lists directly.
		assert_eq!(config.processed, expected_processed);
	}
}
