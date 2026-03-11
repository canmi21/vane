use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use axum::Router;
use seam_server::{SeamError, SeamType, seam_command, seam_procedure};
use seam_server_axum::IntoAxumRouter;
use serde::{Deserialize, Serialize};
use vane_engine::config::ConfigTable;
use vane_engine::engine::{Engine, EngineError};
use vane_primitives::registry::{ConnLayer, ConnPhase, ConnectionState};

#[derive(Clone)]
pub struct VaneState {
	engine: Arc<Engine>,
	started_at: SystemTime,
}

impl VaneState {
	pub const fn new(engine: Arc<Engine>, started_at: SystemTime) -> Self {
		Self { engine, started_at }
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonValue(pub serde_json::Value);

impl SeamType for JsonValue {
	fn jtd_schema() -> serde_json::Value {
		serde_json::json!({})
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, SeamType)]
pub struct EmptyInput {}

#[derive(Debug, Clone, Serialize, Deserialize, SeamType)]
pub struct ListConnectionsOutput {
	pub total: u32,
	pub connections: Vec<ConnectionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, SeamType)]
pub struct ConnectionInfo {
	pub id: String,
	pub peer_addr: String,
	pub server_addr: String,
	pub listen_port: u16,
	pub layer: PanelLayer,
	pub phase: PanelPhase,
	pub protocol: Option<String>,
	pub tls_sni: Option<String>,
	pub tls_version: Option<String>,
	pub forward_target: Option<String>,
	pub started_at_unix_ms: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, SeamType)]
pub enum PanelLayer {
	L4,
	L5,
	L7,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, SeamType)]
pub enum PanelPhase {
	Accepted,
	Detecting,
	Forwarding,
	TlsHandshake,
}

#[derive(Debug, Clone, Serialize, Deserialize, SeamType)]
pub struct GetConfigOutput {
	pub config: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, SeamType)]
pub struct UpdateConfigInput {
	pub config: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, SeamType)]
pub struct UpdateConfigOutput {
	pub ok: bool,
	pub validation_errors: Vec<ValidationIssue>,
	pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, SeamType)]
pub struct ValidationIssue {
	pub port: Option<u16>,
	pub layer: Option<String>,
	pub step_path: Vec<String>,
	pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SeamType)]
pub struct GetSystemInfoOutput {
	pub version: String,
	pub started_at_unix_ms: String,
	pub listener_ports: Vec<u16>,
	pub total_connections: u32,
	pub configured_ports: Vec<u16>,
}

#[allow(clippy::unused_async)]
#[seam_procedure(name = "listConnections", state = VaneState)]
async fn list_connections(
	_input: EmptyInput,
	state: &VaneState,
) -> Result<ListConnectionsOutput, SeamError> {
	let connections =
		state.engine.conn_registry().snapshot().into_iter().map(map_connection).collect::<Vec<_>>();

	Ok(ListConnectionsOutput { total: connections.len().try_into().unwrap_or(u32::MAX), connections })
}

#[allow(clippy::unused_async)]
#[seam_procedure(name = "getConfig", state = VaneState)]
async fn get_config(_input: EmptyInput, state: &VaneState) -> Result<GetConfigOutput, SeamError> {
	let config = serde_json::to_value(state.engine.current_config().as_ref())
		.map_err(|error| SeamError::internal(error.to_string()))?;
	Ok(GetConfigOutput { config: JsonValue(config) })
}

#[allow(clippy::unused_async)]
#[seam_command(name = "updateConfig", state = VaneState)]
async fn update_config(
	input: UpdateConfigInput,
	state: &VaneState,
) -> Result<UpdateConfigOutput, SeamError> {
	let config = serde_json::from_value::<ConfigTable>(input.config.0)
		.map_err(|error| SeamError::validation(error.to_string()))?;

	match state.engine.update_config(config) {
		Ok(()) => Ok(UpdateConfigOutput { ok: true, validation_errors: Vec::new(), error: None }),
		Err(EngineError::ConfigInvalid(errors)) => Ok(UpdateConfigOutput {
			ok: false,
			validation_errors: errors.iter().map(ValidationIssue::from).collect(),
			error: None,
		}),
		Err(error) => Ok(UpdateConfigOutput {
			ok: false,
			validation_errors: Vec::new(),
			error: Some(error.to_string()),
		}),
	}
}

#[allow(clippy::unused_async)]
#[seam_procedure(name = "getSystemInfo", state = VaneState)]
async fn get_system_info(
	_input: EmptyInput,
	state: &VaneState,
) -> Result<GetSystemInfoOutput, SeamError> {
	let listener_ports =
		state.engine.listeners().iter().map(|handle| handle.local_addr().port()).collect();
	let mut configured_ports =
		state.engine.current_config().ports.keys().copied().collect::<Vec<_>>();
	configured_ports.sort_unstable();

	Ok(GetSystemInfoOutput {
		version: env!("CARGO_PKG_VERSION").to_owned(),
		started_at_unix_ms: system_time_to_unix_ms(state.started_at),
		listener_ports,
		total_connections: state.engine.conn_registry().count().try_into().unwrap_or(u32::MAX),
		configured_ports,
	})
}

pub fn build_panel_router(state: Arc<VaneState>) -> Router {
	seam_server::SeamServer::new()
		.procedure(list_connections_procedure(Arc::clone(&state)))
		.procedure(get_config_procedure(Arc::clone(&state)))
		.procedure(get_system_info_procedure(Arc::clone(&state)))
		.procedure(update_config_procedure(state))
		.into_axum_router()
}

pub async fn start_panel_server(
	state: Arc<VaneState>,
	bind_addr: SocketAddr,
) -> Result<(), std::io::Error> {
	let listener = tokio::net::TcpListener::bind(bind_addr).await?;
	let local_addr = listener.local_addr()?;
	tracing::info!(%local_addr, "panel server listening");
	axum::serve(listener, build_panel_router(state)).await
}

fn map_connection(state: ConnectionState) -> ConnectionInfo {
	ConnectionInfo {
		id: state.id,
		peer_addr: state.peer_addr.to_string(),
		server_addr: state.server_addr.to_string(),
		listen_port: state.server_addr.port(),
		layer: match state.layer {
			ConnLayer::L4 => PanelLayer::L4,
			ConnLayer::L5 => PanelLayer::L5,
			ConnLayer::L7 => PanelLayer::L7,
		},
		phase: match state.phase {
			ConnPhase::Accepted => PanelPhase::Accepted,
			ConnPhase::Detecting => PanelPhase::Detecting,
			ConnPhase::Forwarding => PanelPhase::Forwarding,
			ConnPhase::TlsHandshake => PanelPhase::TlsHandshake,
		},
		protocol: state.protocol,
		tls_sni: state.tls_sni,
		tls_version: state.tls_version,
		forward_target: state.forward_target.map(|addr| addr.to_string()),
		started_at_unix_ms: started_instant_to_unix_ms(state.started_at),
	}
}

fn started_instant_to_unix_ms(started_at: std::time::Instant) -> String {
	let elapsed = started_at.elapsed();
	let now = SystemTime::now();
	let started_time = now.checked_sub(elapsed).unwrap_or(now);
	system_time_to_unix_ms(started_time)
}

fn system_time_to_unix_ms(time: SystemTime) -> String {
	time
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or(std::time::Duration::ZERO)
		.as_millis()
		.to_string()
}

impl From<&vane_engine::config::ValidationError> for ValidationIssue {
	fn from(value: &vane_engine::config::ValidationError) -> Self {
		Self {
			port: value.port,
			layer: value.layer.map(|layer| layer.to_string()),
			step_path: value.step_path.clone(),
			message: value.message.clone(),
		}
	}
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
	use super::*;

	use axum::body::{Body, to_bytes};
	use axum::http::{Request, StatusCode};
	use tower::ServiceExt;
	use vane_engine::flow::default_plugin_registry;
	use vane_transport::tls::CertStore;

	#[tokio::test]
	async fn get_system_info_returns_expected_json() {
		let mut engine =
			Engine::new(ConfigTable::default(), default_plugin_registry(), CertStore::new())
				.expect("engine should build with empty config");
		engine.start().await.expect("empty config start should succeed");

		let state = Arc::new(VaneState::new(Arc::new(engine), SystemTime::now()));
		let router = build_panel_router(state);
		let response = router
			.oneshot(
				Request::builder()
					.method("POST")
					.uri("/_seam/procedure/getSystemInfo")
					.header("content-type", "application/json")
					.body(Body::from("{}"))
					.expect("request should build"),
			)
			.await
			.expect("router should respond");

		assert_eq!(response.status(), StatusCode::OK);
		let body = to_bytes(response.into_body(), usize::MAX).await.expect("body should read");
		let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be valid json");
		assert_eq!(json["ok"], true);
		assert_eq!(json["data"]["listener_ports"], serde_json::json!([]));
		assert_eq!(json["data"]["configured_ports"], serde_json::json!([]));
		assert!(json["data"]["started_at_unix_ms"].as_str().is_some());
	}
}
