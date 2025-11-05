/* engine/src/modules/router/generate.rs */

use crate::{daemon::config, modules::domain::entrance as domain_helper};
use fancy_log::{LogLevel, log};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// --- Data Structures for Deserializing layout.json ---

#[derive(Deserialize, Debug)]
struct LayoutNode {
	id: String,
	#[serde(rename = "type")]
	node_type: String,
	data: Value,
	// Add the optional 'variables' field.
	#[serde(default)]
	variables: Value,
	// Add the optional 'version' field.
	#[serde(default)]
	version: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Connection {
	from_node_id: String,
	from_handle: String,
	to_node_id: String,
}

#[derive(Deserialize, Debug)]
struct LayoutConfig {
	nodes: Vec<LayoutNode>,
	connections: Vec<Connection>,
}

// --- Data Structures for Serializing router.gen ---

#[derive(Serialize, Debug)]
struct TreeNode {
	#[serde(rename = "type")]
	node_type: String,
	// Add the 'version' field to the output tree.
	// It will not be serialized if it's an empty string.
	#[serde(skip_serializing_if = "String::is_empty")]
	version: String,
	data: Value,
	// Add the 'variables' field to the output tree.
	#[serde(skip_serializing_if = "Value::is_null")]
	variables: Value,
	// The `next` field contains branches, keyed by the output handle name.
	next: HashMap<String, Box<TreeNode>>,
}

// --- Core Logic ---

/// Generates a hierarchical router tree from a flat layout.json file.
/// The resulting tree is saved to `router.gen` in the domain's directory.
pub async fn generate_router_tree(domain: &str) {
	log(
		LogLevel::Debug,
		&format!("Generating router tree for domain: {}", domain),
	);

	let layout_path = config::get_config_dir()
		.join(domain_helper::domain_to_dir_name(domain))
		.join("layout.json");

	let content = match tokio::fs::read_to_string(&layout_path).await {
		Ok(c) => c,
		Err(_) => {
			log(
				LogLevel::Warn,
				&format!("layout.json not found for {}, skipping generation.", domain),
			);
			return;
		}
	};

	if content.trim().is_empty() || content.trim() == "{}" {
		log(
			LogLevel::Debug,
			&format!("layout.json for {} is empty, skipping generation.", domain),
		);
		return;
	}

	let layout: LayoutConfig = match serde_json::from_str(&content) {
		Ok(l) => l,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("Failed to parse layout.json for {}: {}", domain, e),
			);
			return;
		}
	};

	let nodes_map: HashMap<String, LayoutNode> = layout
		.nodes
		.into_iter()
		.map(|n| (n.id.clone(), n))
		.collect();

	let connections_map: HashMap<(String, String), String> = layout
		.connections
		.into_iter()
		.map(|c| ((c.from_node_id, c.from_handle), c.to_node_id))
		.collect();

	if let Some(root_node) = nodes_map.get("entry-point") {
		if let Some(first_node_id) = connections_map.get(&(root_node.id.clone(), "output".to_string()))
		{
			let mut visited = HashSet::new();
			if let Some(tree) = build_node_tree(first_node_id, &nodes_map, &connections_map, &mut visited)
			{
				let router_gen_path = config::get_config_dir()
					.join(domain_helper::domain_to_dir_name(domain))
					.join("router.gen");

				match serde_json::to_string_pretty(&tree) {
					Ok(tree_json) => {
						if let Err(e) = tokio::fs::write(&router_gen_path, tree_json).await {
							log(
								LogLevel::Error,
								&format!("Failed to write router.gen for {}: {}", domain, e),
							);
						} else {
							log(
								LogLevel::Info,
								&format!("Successfully generated router tree for {}", domain),
							);
						}
					}
					Err(e) => {
						log(
							LogLevel::Error,
							&format!("Failed to serialize router tree for {}: {}", domain, e),
						);
					}
				}
			}
		}
	} else {
		log(
			LogLevel::Warn,
			&format!(
				"No 'entry-point' node found in layout.json for {}, cannot generate router tree.",
				domain
			),
		);
	}
}

/// Recursive helper function to build the tree for a given node ID.
fn build_node_tree(
	node_id: &str,
	nodes_map: &HashMap<String, LayoutNode>,
	connections_map: &HashMap<(String, String), String>,
	visited: &mut HashSet<String>,
) -> Option<Box<TreeNode>> {
	if visited.contains(node_id) {
		log(
			LogLevel::Warn,
			&format!(
				"Cycle detected in layout graph at node '{}', stopping branch.",
				node_id
			),
		);
		return None;
	}
	visited.insert(node_id.to_string());

	if let Some(current_node) = nodes_map.get(node_id) {
		let mut next_branches = HashMap::new();

		for ((from_node_id, from_handle), to_node_id) in connections_map {
			if from_node_id == node_id {
				if let Some(next_node_tree) =
					build_node_tree(to_node_id, nodes_map, connections_map, visited)
				{
					next_branches.insert(from_handle.clone(), next_node_tree);
				}
			}
		}

		visited.remove(node_id);

		return Some(Box::new(TreeNode {
			node_type: current_node.node_type.clone(),
			version: current_node.version.clone(),
			data: current_node.data.clone(),
			variables: current_node.variables.clone(),
			next: next_branches,
		}));
	}

	None
}
