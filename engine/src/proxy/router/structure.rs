/* engine/src/proxy/router/structure.rs */

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Represents the executable, tree-like structure of a domain's router.
/// This is the in-memory representation of a `router.gen` file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouterNode {
	#[serde(rename = "type")]
	pub node_type: String,

	#[serde(skip_serializing_if = "String::is_empty", default)]
	pub version: String,

	pub data: Value,

	#[serde(skip_serializing_if = "Value::is_null", default)]
	pub variables: Value,

	#[serde(default)]
	pub next: HashMap<String, Box<RouterNode>>,
}
