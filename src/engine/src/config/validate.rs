use std::collections::HashSet;
use std::fmt;
use std::net::IpAddr;

use crate::flow::{PluginAction, PluginRegistry};

use super::{ConfigPatch, ConfigTable, FlowNode, Layer, PortConfig, TerminationAction};

/// A single validation failure with location context.
#[derive(Debug, Clone)]
pub struct ValidationError {
	pub port: Option<u16>,
	pub layer: Option<Layer>,
	pub step_path: Vec<String>,
	pub message: String,
}

impl fmt::Display for ValidationError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		if let Some(port) = self.port {
			write!(f, "port {port}")?;
		}
		if let Some(layer) = &self.layer {
			if self.port.is_some() {
				write!(f, ", ")?;
			}
			write!(f, "{layer}")?;
		}
		if !self.step_path.is_empty() {
			if self.port.is_some() || self.layer.is_some() {
				write!(f, ", ")?;
			}
			write!(f, "{}", self.step_path.join(" > "))?;
		}
		write!(f, ": {}", self.message)
	}
}

/// Bundles validation state to avoid too-many-arguments.
struct ValidateCtx<'a> {
	registry: &'a PluginRegistry,
	errors: &'a mut Vec<ValidationError>,
	port: u16,
	layer: Layer,
	port_config: &'a PortConfig,
}

impl ConfigTable {
	/// Validate the entire config table against a plugin registry.
	/// Returns `Ok(())` if valid, or `Err` with all collected errors.
	pub fn validate(&self, registry: &PluginRegistry) -> Result<(), Vec<ValidationError>> {
		let mut errors = Vec::new();

		if self.ports.is_empty() {
			errors.push(ValidationError {
				port: None,
				layer: None,
				step_path: vec![],
				message: "ports map must not be empty".to_owned(),
			});
		}

		for (&port, port_config) in &self.ports {
			// L4 flow
			let mut ctx =
				ValidateCtx { registry, errors: &mut errors, port, layer: Layer::L4, port_config };
			let mut path = Vec::new();
			let mut ancestors = HashSet::new();
			validate_node(&port_config.l4, &mut ctx, &mut path, &mut ancestors);

			// L5 flow
			if let Some(l5) = &port_config.l5 {
				if !self.certs.contains_key(&l5.default_cert) {
					errors.push(ValidationError {
						port: Some(port),
						layer: Some(Layer::L5),
						step_path: vec![],
						message: format!("default_cert {:?} not found in certs map", l5.default_cert),
					});
				}
				let mut ctx =
					ValidateCtx { registry, errors: &mut errors, port, layer: Layer::L5, port_config };
				let mut path = Vec::new();
				let mut ancestors = HashSet::new();
				validate_node(&l5.flow, &mut ctx, &mut path, &mut ancestors);
			}

			// L7 flow
			if let Some(l7) = &port_config.l7 {
				let mut ctx =
					ValidateCtx { registry, errors: &mut errors, port, layer: Layer::L7, port_config };
				let mut path = Vec::new();
				let mut ancestors = HashSet::new();
				validate_node(&l7.flow, &mut ctx, &mut path, &mut ancestors);
			}
		}

		if errors.is_empty() { Ok(()) } else { Err(errors) }
	}

	/// Apply a partial patch, validate the result, and return the merged config.
	pub fn merge_update(
		&self,
		patch: ConfigPatch,
		registry: &PluginRegistry,
	) -> Result<Self, Vec<ValidationError>> {
		let mut merged = self.clone();

		if let Some(ports) = patch.ports {
			for (port, config) in ports {
				merged.ports.insert(port, config);
			}
		}
		if let Some(global) = patch.global {
			merged.global = global;
		}
		if let Some(certs) = patch.certs {
			for (name, entry) in certs {
				merged.certs.insert(name, entry);
			}
		}

		merged.validate(registry)?;
		Ok(merged)
	}
}

fn validate_node(
	node: &FlowNode,
	ctx: &mut ValidateCtx<'_>,
	path: &mut Vec<String>,
	ancestors: &mut HashSet<String>,
) {
	path.push(node.plugin.clone());

	// Cycle detection
	if !ancestors.insert(node.plugin.clone()) {
		ctx.errors.push(ValidationError {
			port: Some(ctx.port),
			layer: Some(ctx.layer),
			step_path: path.clone(),
			message: format!("cycle detected: {:?} appears in its own ancestry", node.plugin),
		});
		path.pop();
		return;
	}

	// Plugin existence
	let Some(action) = ctx.registry.get(&node.plugin) else {
		ctx.errors.push(ValidationError {
			port: Some(ctx.port),
			layer: Some(ctx.layer),
			step_path: path.clone(),
			message: format!("plugin {:?} not found in registry", node.plugin),
		});
		ancestors.remove(&node.plugin);
		path.pop();
		return;
	};

	match action {
		PluginAction::Middleware(_) => {
			if node.branches.is_empty() {
				ctx.errors.push(ValidationError {
					port: Some(ctx.port),
					layer: Some(ctx.layer),
					step_path: path.clone(),
					message: "middleware must have at least one branch".to_owned(),
				});
			}
			if node.termination.is_some() {
				ctx.errors.push(ValidationError {
					port: Some(ctx.port),
					layer: Some(ctx.layer),
					step_path: path.clone(),
					message: "middleware must not have termination".to_owned(),
				});
			}
		}
		PluginAction::Terminator(_) => {
			if !node.branches.is_empty() {
				ctx.errors.push(ValidationError {
					port: Some(ctx.port),
					layer: Some(ctx.layer),
					step_path: path.clone(),
					message: "terminator must not have branches".to_owned(),
				});
			}
			validate_termination(node, ctx, path);
		}
	}

	// Param validation
	validate_params(&node.params, ctx, path);

	// Recurse into branches
	for child in node.branches.values() {
		validate_node(child, ctx, path, ancestors);
	}

	ancestors.remove(&node.plugin);
	path.pop();
}

fn validate_termination(node: &FlowNode, ctx: &mut ValidateCtx<'_>, path: &[String]) {
	let Some(TerminationAction::Upgrade { target_layer }) = &node.termination else {
		return;
	};

	match target_layer {
		Layer::L4 => {
			ctx.errors.push(ValidationError {
				port: Some(ctx.port),
				layer: Some(ctx.layer),
				step_path: path.to_vec(),
				message: "cannot upgrade to L4".to_owned(),
			});
		}
		Layer::L5 => {
			if ctx.layer == Layer::L5 {
				ctx.errors.push(ValidationError {
					port: Some(ctx.port),
					layer: Some(ctx.layer),
					step_path: path.to_vec(),
					message: "L5 flow cannot upgrade to L5".to_owned(),
				});
			} else if ctx.port_config.l5.is_none() {
				ctx.errors.push(ValidationError {
					port: Some(ctx.port),
					layer: Some(ctx.layer),
					step_path: path.to_vec(),
					message: "upgrade to L5 but no l5 config defined".to_owned(),
				});
			}
		}
		Layer::L7 => {
			if ctx.layer == Layer::L7 {
				ctx.errors.push(ValidationError {
					port: Some(ctx.port),
					layer: Some(ctx.layer),
					step_path: path.to_vec(),
					message: "L7 flow cannot have upgrade".to_owned(),
				});
			} else if ctx.port_config.l7.is_none() {
				ctx.errors.push(ValidationError {
					port: Some(ctx.port),
					layer: Some(ctx.layer),
					step_path: path.to_vec(),
					message: "upgrade to L7 but no l7 config defined".to_owned(),
				});
			}
		}
	}
}

fn validate_params(params: &serde_json::Value, ctx: &mut ValidateCtx<'_>, path: &[String]) {
	if let Some(ip) = params.get("ip").and_then(serde_json::Value::as_str)
		&& ip.parse::<IpAddr>().is_err()
	{
		ctx.errors.push(ValidationError {
			port: Some(ctx.port),
			layer: Some(ctx.layer),
			step_path: path.to_vec(),
			message: format!("params.ip {ip:?} is not a valid IP address"),
		});
	}
	if let Some(port_val) = params.get("port").and_then(serde_json::Value::as_u64)
		&& u16::try_from(port_val).is_err()
	{
		ctx.errors.push(ValidationError {
			port: Some(ctx.port),
			layer: Some(ctx.layer),
			step_path: path.to_vec(),
			message: format!("params.port {port_val} does not fit u16"),
		});
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
	use std::future::Future;
	use std::net::SocketAddr;
	use std::pin::Pin;

	use vane_primitives::kv::KvStore;
	use vane_transport::stream::ConnectionStream;

	use std::collections::HashMap;

	use super::*;
	use crate::config::{CertEntry, GlobalConfig, L5Config, L7Config, ListenConfig};
	use crate::flow::{BranchAction, ExecutionContext, Middleware, Terminator};

	struct MockMiddleware;
	impl Middleware for MockMiddleware {
		fn execute(
			&self,
			_params: &serde_json::Value,
			_ctx: &dyn ExecutionContext,
		) -> Result<BranchAction, anyhow::Error> {
			Ok(BranchAction { branch: "default".to_owned(), updates: vec![] })
		}
	}

	struct MockTerminator;
	impl Terminator for MockTerminator {
		fn execute(
			&self,
			_params: &serde_json::Value,
			_kv: &KvStore,
			_stream: ConnectionStream,
			_peer_addr: SocketAddr,
			_server_addr: SocketAddr,
		) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>> {
			Box::pin(async { Ok(()) })
		}
	}

	fn mock_registry() -> PluginRegistry {
		PluginRegistry::new()
			.register("echo.branch", PluginAction::Middleware(Box::new(MockMiddleware)))
			.register("tcp.forward", PluginAction::Terminator(Box::new(MockTerminator)))
	}

	fn simple_middleware(branches: HashMap<String, FlowNode>) -> FlowNode {
		FlowNode {
			plugin: "echo.branch".to_owned(),
			params: serde_json::Value::default(),
			branches,
			termination: None,
		}
	}

	fn simple_terminator() -> FlowNode {
		FlowNode {
			plugin: "tcp.forward".to_owned(),
			params: serde_json::json!({"ip": "127.0.0.1", "port": 8080}),
			branches: HashMap::new(),
			termination: Some(TerminationAction::Finished),
		}
	}

	fn simple_port_l4_only(l4: FlowNode) -> PortConfig {
		PortConfig { listen: ListenConfig::default(), l4, l5: None, l7: None }
	}

	fn simple_config(ports: HashMap<u16, PortConfig>) -> ConfigTable {
		ConfigTable { ports, global: GlobalConfig::default(), certs: HashMap::new() }
	}

	// Test 1: valid single layer
	#[test]
	fn valid_single_layer() {
		let l4 = simple_middleware(HashMap::from([("default".to_owned(), simple_terminator())]));
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		assert!(config.validate(&mock_registry()).is_ok());
	}

	// Test 2: valid multi-layer
	#[test]
	fn valid_multi_layer() {
		let mut l4_term = simple_terminator();
		l4_term.termination = Some(TerminationAction::Upgrade { target_layer: Layer::L5 });

		let l4 = simple_middleware(HashMap::from([("default".to_owned(), l4_term)]));

		let mut l5_term = simple_terminator();
		l5_term.termination = Some(TerminationAction::Upgrade { target_layer: Layer::L7 });

		let l5 = L5Config { default_cert: "main".to_owned(), alpn: vec![], flow: l5_term };

		let l7 = L7Config { flow: simple_terminator() };

		let port_config =
			PortConfig { listen: ListenConfig::default(), l4, l5: Some(l5), l7: Some(l7) };

		let config = ConfigTable {
			ports: HashMap::from([(443, port_config)]),
			global: GlobalConfig::default(),
			certs: HashMap::from([(
				"main".to_owned(),
				CertEntry::File { cert_path: "/cert.pem".to_owned(), key_path: "/key.pem".to_owned() },
			)]),
		};
		assert!(config.validate(&mock_registry()).is_ok());
	}

	// Test 3: upgrade L5 missing config
	#[test]
	fn upgrade_l5_missing_config() {
		let mut term = simple_terminator();
		term.termination = Some(TerminationAction::Upgrade { target_layer: Layer::L5 });

		let l4 = simple_middleware(HashMap::from([("default".to_owned(), term)]));
		// No l5 config
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("upgrade to L5")));
	}

	// Test 4: middleware empty branches
	#[test]
	fn middleware_empty_branches() {
		let l4 = FlowNode {
			plugin: "echo.branch".to_owned(),
			params: serde_json::Value::default(),
			branches: HashMap::new(),
			termination: None,
		};
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("at least one branch")));
	}

	// Test 5: ancestor cycle detected
	#[test]
	fn ancestor_cycle_detected() {
		// echo.branch -> branch "loop" -> echo.branch (cycle)
		let inner = FlowNode {
			plugin: "echo.branch".to_owned(),
			params: serde_json::Value::default(),
			branches: HashMap::from([("x".to_owned(), simple_terminator())]),
			termination: None,
		};
		let outer = FlowNode {
			plugin: "echo.branch".to_owned(),
			params: serde_json::Value::default(),
			branches: HashMap::from([("loop".to_owned(), inner)]),
			termination: None,
		};
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(outer))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("cycle detected")));
	}

	// Test 6: merge then validate
	#[test]
	fn merge_then_validate() {
		let l4 = simple_middleware(HashMap::from([("default".to_owned(), simple_terminator())]));
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4.clone()))]));

		let patch = ConfigPatch {
			ports: Some(HashMap::from([(443, simple_port_l4_only(l4))])),
			global: None,
			certs: None,
		};

		let merged = config.merge_update(patch, &mock_registry()).unwrap();
		assert!(merged.ports.contains_key(&80));
		assert!(merged.ports.contains_key(&443));
	}

	// Test 7: json serde roundtrip (in types.rs already, but covering full validate path)
	#[test]
	fn json_serde_roundtrip() {
		let l4 = simple_middleware(HashMap::from([("default".to_owned(), simple_terminator())]));
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let json = serde_json::to_string(&config).unwrap();
		let back: ConfigTable = serde_json::from_str(&json).unwrap();
		assert_eq!(config, back);
		assert!(back.validate(&mock_registry()).is_ok());
	}

	// Test 8: plugin not found
	#[test]
	fn plugin_not_found() {
		let l4 = FlowNode {
			plugin: "nonexistent.plugin".to_owned(),
			params: serde_json::Value::default(),
			branches: HashMap::new(),
			termination: None,
		};
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("not found in registry")));
	}

	// Test 9: L5 cert missing
	#[test]
	fn l5_cert_missing() {
		let mut term = simple_terminator();
		term.termination = Some(TerminationAction::Upgrade { target_layer: Layer::L5 });

		let l4 = simple_middleware(HashMap::from([("default".to_owned(), term)]));

		let l5 =
			L5Config { default_cert: "missing_cert".to_owned(), alpn: vec![], flow: simple_terminator() };

		let port_config = PortConfig { listen: ListenConfig::default(), l4, l5: Some(l5), l7: None };

		let config = ConfigTable {
			ports: HashMap::from([(443, port_config)]),
			global: GlobalConfig::default(),
			certs: HashMap::new(), // no certs
		};
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("not found in certs")));
	}

	// Test 10: terminator with branches
	#[test]
	fn terminator_with_branches() {
		let l4 = FlowNode {
			plugin: "tcp.forward".to_owned(),
			params: serde_json::json!({"ip": "127.0.0.1", "port": 8080}),
			branches: HashMap::from([("bad".to_owned(), simple_terminator())]),
			termination: Some(TerminationAction::Finished),
		};
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("must not have branches")));
	}

	// Test 11: L7 upgrade forbidden
	#[test]
	fn l7_upgrade_forbidden() {
		let l4_term = simple_terminator();
		let l4 = simple_middleware(HashMap::from([("default".to_owned(), l4_term)]));

		let mut l7_term = simple_terminator();
		l7_term.termination = Some(TerminationAction::Upgrade { target_layer: Layer::L7 });

		let l7 = L7Config { flow: l7_term };

		let port_config = PortConfig { listen: ListenConfig::default(), l4, l5: None, l7: Some(l7) };
		let config = simple_config(HashMap::from([(80, port_config)]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("L7 flow cannot have upgrade")));
	}

	#[test]
	fn validation_error_display() {
		let err = ValidationError {
			port: Some(443),
			layer: Some(Layer::L4),
			step_path: vec!["echo.branch".to_owned(), "tcp.forward".to_owned()],
			message: "plugin not found".to_owned(),
		};
		let display = err.to_string();
		assert_eq!(display, "port 443, L4, echo.branch > tcp.forward: plugin not found");
	}

	#[test]
	fn empty_ports_error() {
		let config = simple_config(HashMap::new());
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("ports map must not be empty")));
	}

	#[test]
	fn params_invalid_ip() {
		let mut term = simple_terminator();
		term.params = serde_json::json!({"ip": "not-an-ip", "port": 80});
		let l4 = simple_middleware(HashMap::from([("default".to_owned(), term)]));
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("not a valid IP")));
	}

	#[test]
	fn params_port_out_of_range() {
		let mut term = simple_terminator();
		term.params = serde_json::json!({"ip": "127.0.0.1", "port": 99999});
		let l4 = simple_middleware(HashMap::from([("default".to_owned(), term)]));
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("does not fit u16")));
	}

	#[test]
	fn upgrade_to_l4_error() {
		let mut term = simple_terminator();
		term.termination = Some(TerminationAction::Upgrade { target_layer: Layer::L4 });
		let l4 = simple_middleware(HashMap::from([("default".to_owned(), term)]));
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("cannot upgrade to L4")));
	}

	#[test]
	fn middleware_with_termination_error() {
		let l4 = FlowNode {
			plugin: "echo.branch".to_owned(),
			params: serde_json::Value::default(),
			branches: HashMap::from([("default".to_owned(), simple_terminator())]),
			termination: Some(TerminationAction::Finished),
		};
		let config = simple_config(HashMap::from([(80, simple_port_l4_only(l4))]));
		let errors = config.validate(&mock_registry()).unwrap_err();
		assert!(errors.iter().any(|e| e.message.contains("must not have termination")));
	}

	#[test]
	fn validation_error_display_no_port() {
		let err = ValidationError {
			port: None,
			layer: Some(Layer::L4),
			step_path: vec!["tcp.forward".to_owned()],
			message: "test error".to_owned(),
		};
		let display = err.to_string();
		// Should not have "port" prefix, should start with layer
		assert_eq!(display, "L4, tcp.forward: test error");
	}
}
