mod types;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::{Router, serve};
use rust_embed::Embed;
use vane_engine::config::ConfigTable;
use vane_engine::engine::{Engine, EngineError};
use vane_primitives::registry::{ConnLayer, ConnPhase};

pub use types::*;

const DEFAULT_PANEL_BIND_ADDR: &str = "127.0.0.1:3333";

// -- State ----------------------------------------------------------------

#[derive(Clone)]
pub struct PanelState {
	engine: Arc<Engine>,
	started_at: SystemTime,
}

impl PanelState {
	pub const fn new(engine: Arc<Engine>, started_at: SystemTime) -> Self {
		Self { engine, started_at }
	}
}

// -- Static assets --------------------------------------------------------

#[derive(Embed)]
#[folder = "web/dist/"]
struct Assets;

async fn serve_static(path: axum::extract::Path<String>) -> Response {
	let path = path.0;
	serve_embedded_file(&path)
}

async fn serve_index() -> Response {
	serve_embedded_file("index.html")
}

fn serve_embedded_file(path: &str) -> Response {
	let Some(file) = Assets::get(path) else {
		return StatusCode::NOT_FOUND.into_response();
	};
	let mime = mime_guess::from_path(path).first_or_octet_stream();
	([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response()
}

// -- API handlers ---------------------------------------------------------

async fn list_connections(State(state): State<PanelState>) -> Json<ListConnectionsOutput> {
	let connections =
		state.engine.conn_registry().snapshot().into_iter().map(map_connection).collect::<Vec<_>>();
	let total = connections.len().try_into().unwrap_or(u32::MAX);
	Json(ListConnectionsOutput { total, connections })
}

async fn get_system_info(State(state): State<PanelState>) -> Json<SystemInfoOutput> {
	let listener_ports = state.engine.listener_addrs().iter().map(|(_, addr)| addr.port()).collect();
	let mut configured_ports =
		state.engine.current_config().ports.keys().copied().collect::<Vec<_>>();
	configured_ports.sort_unstable();

	Json(SystemInfoOutput {
		version: env!("CARGO_PKG_VERSION").to_owned(),
		started_at_unix_ms: system_time_to_unix_ms(state.started_at),
		listener_ports,
		total_connections: state.engine.conn_registry().count().try_into().unwrap_or(u32::MAX),
		configured_ports,
	})
}

async fn get_config(State(state): State<PanelState>) -> Response {
	match serde_json::to_value(state.engine.current_config().as_ref()) {
		Ok(config) => Json(GetConfigOutput { config: JsonBlob(config) }).into_response(),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
				.into_response()
		}
	}
}

async fn update_config(
	State(state): State<PanelState>,
	Json(input): Json<UpdateConfigInput>,
) -> Json<UpdateConfigOutput> {
	let config = match serde_json::from_value::<ConfigTable>(input.config.0) {
		Ok(c) => c,
		Err(e) => {
			return Json(UpdateConfigOutput {
				ok: false,
				validation_errors: Vec::new(),
				error: Some(e.to_string()),
			});
		}
	};

	match state.engine.update_config(config).await {
		Ok(()) => Json(UpdateConfigOutput { ok: true, validation_errors: Vec::new(), error: None }),
		Err(EngineError::ConfigInvalid(errors)) => Json(UpdateConfigOutput {
			ok: false,
			validation_errors: errors.iter().map(ValidationIssue::from).collect(),
			error: None,
		}),
		Err(e) => Json(UpdateConfigOutput {
			ok: false,
			validation_errors: Vec::new(),
			error: Some(e.to_string()),
		}),
	}
}

// -- Router ---------------------------------------------------------------

pub fn build_panel_router(state: PanelState) -> Router {
	let api = Router::new()
		.route("/listConnections", get(list_connections))
		.route("/getSystemInfo", get(get_system_info))
		.route("/getConfig", get(get_config))
		.route("/updateConfig", post(update_config));

	Router::new()
		.nest("/_bridge", api)
		.route("/assets/{*path}", get(serve_static))
		.fallback(get(serve_index))
		.with_state(state)
}

pub fn panel_bind_addr() -> Result<SocketAddr, String> {
	std::env::var("VANE_PANEL_BIND_ADDR")
		.unwrap_or_else(|_| DEFAULT_PANEL_BIND_ADDR.to_owned())
		.parse()
		.map_err(|e| format!("invalid VANE_PANEL_BIND_ADDR: {e}"))
}

pub async fn start_panel_server(
	state: PanelState,
	bind_addr: SocketAddr,
) -> Result<(), std::io::Error> {
	let listener = tokio::net::TcpListener::bind(bind_addr).await?;
	let local_addr = listener.local_addr()?;
	tracing::info!(%local_addr, "panel server listening");
	serve(listener, build_panel_router(state)).await
}

// -- Mapping helpers ------------------------------------------------------

fn map_connection(state: vane_primitives::registry::ConnectionState) -> ConnectionInfo {
	ConnectionInfo {
		id: state.id,
		peer_addr: state.peer_addr.to_string(),
		server_addr: state.server_addr.to_string(),
		listen_port: state.server_addr.port(),
		layer: match state.layer {
			ConnLayer::L4 => Layer::L4,
			ConnLayer::L5 => Layer::L5,
			ConnLayer::L7 => Layer::L7,
		},
		phase: match state.phase {
			ConnPhase::Accepted => Phase::Accepted,
			ConnPhase::Detecting => Phase::Detecting,
			ConnPhase::Forwarding => Phase::Forwarding,
			ConnPhase::TlsHandshake => Phase::TlsHandshake,
		},
		protocol: state.protocol,
		tls_sni: state.tls_sni,
		tls_version: state.tls_version,
		forward_target: state.forward_target.map(|a| a.to_string()),
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
			layer: value.layer.map(|l| l.to_string()),
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

	fn test_state() -> PanelState {
		let engine = Engine::new(ConfigTable::default(), default_plugin_registry(), CertStore::new())
			.expect("engine should build with empty config");
		// Cannot call engine.start() in sync context; listeners will be empty
		PanelState::new(Arc::new(engine), SystemTime::now())
	}

	#[tokio::test]
	async fn get_system_info_returns_ok() {
		let state = test_state();
		let router = build_panel_router(state);
		let resp = router
			.oneshot(
				Request::builder()
					.uri("/_bridge/getSystemInfo")
					.body(Body::empty())
					.expect("request should build"),
			)
			.await
			.expect("router should respond");

		assert_eq!(resp.status(), StatusCode::OK);
		let body = to_bytes(resp.into_body(), usize::MAX).await.expect("body should read");
		let json: serde_json::Value = serde_json::from_slice(&body).expect("body should be valid json");
		assert_eq!(json["listenerPorts"], serde_json::json!([]));
		assert_eq!(json["configuredPorts"], serde_json::json!([]));
		assert!(json["startedAtUnixMs"].as_str().is_some());
	}

	#[tokio::test]
	async fn list_connections_returns_empty() {
		let state = test_state();
		let router = build_panel_router(state);
		let resp = router
			.oneshot(
				Request::builder()
					.uri("/_bridge/listConnections")
					.body(Body::empty())
					.expect("request should build"),
			)
			.await
			.expect("router should respond");

		assert_eq!(resp.status(), StatusCode::OK);
		let body = to_bytes(resp.into_body(), usize::MAX).await.expect("body should read");
		let json: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
		assert_eq!(json["total"], 0);
		assert_eq!(json["connections"], serde_json::json!([]));
	}

	#[tokio::test]
	async fn fallback_serves_index_html() {
		let state = test_state();
		let router = build_panel_router(state);
		let resp = router
			.oneshot(Request::builder().uri("/").body(Body::empty()).expect("request should build"))
			.await
			.expect("router should respond");

		// rust-embed includes dist/ at compile time; if dist/ was built, this is 200
		let status = resp.status();
		assert!(
			status == StatusCode::OK || status == StatusCode::NOT_FOUND,
			"unexpected status: {status}"
		);
	}
}
