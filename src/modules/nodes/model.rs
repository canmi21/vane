/* src/modules/nodes/model.rs */

use crate::modules::stack::transport::loader::PreProcess;
use arc_swap::ArcSwap;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use validator::{Validate, ValidationError, ValidationErrors};

lazy_static! {
	pub static ref NODES_STATE: ArcSwap<NodesConfig> = ArcSwap::default();
	static ref NAME_REGEX: regex::Regex = regex::Regex::new(r"^[a-z0-9-]+$").unwrap();
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IpType {
	Ipv4,
	Ipv6,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq, Hash)]
pub struct IpConfig {
	#[validate(ip)]
	pub address: String,
	#[validate(length(min = 1, message = "must have at least one port"))]
	#[serde(default)]
	pub ports: Vec<u16>,
	pub r#type: IpType,
}

#[derive(Serialize, Deserialize, Debug, Clone, Validate, PartialEq, Eq, Hash)]
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
		let mut result = Ok(());
		for (i, node) in self.nodes.iter().enumerate() {
			let field_name = Box::leak(format!("nodes[{}]", i).into_boxed_str());
			result = ValidationErrors::merge(result, field_name, node.validate());
		}
		if let Err(e) = validate_unique_node_names(&self.nodes) {
			if let Err(ref mut errors) = result {
				errors.add("nodes", e);
			} else {
				let mut errors = ValidationErrors::new();
				errors.add("nodes", e);
				result = Err(errors);
			}
		}
		result
	}
}

impl PreProcess for NodesConfig {
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
